#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>
#include <unistd.h>

static NSInteger trackingGeneration;
static BOOL tracking;
static NSInteger sampleToken;
static NSInteger sampleResult;
static CGWindowID trackedWindow;
static NSInteger statusWindow;
static NSURL *outputDirectory;
static int traceFD = -1;

static void traceEvent(const char *phase, const char *event) {
    char record[96];
    int length = snprintf(record, sizeof(record), "%s %s\n", phase, event);
    if (traceFD >= 0 && length > 0 && (size_t)length < sizeof(record)) {
        (void)write(traceFD, record, (size_t)length);
    }
}

static const char *trackingPhase(void) {
    switch (trackingGeneration) {
        case 1: return "first-menu";
        case 2: return "reopen-menu";
        case 3: return "retry-menu";
        default: return "other-menu";
    }
}

static void onMainDuringTracking(dispatch_block_t block) {
    CFRunLoopRef main = CFRunLoopGetMain();
    NSArray *modes = @[NSEventTrackingRunLoopMode, NSRunLoopCommonModes];
    CFRunLoopPerformBlock(main, (__bridge CFArrayRef)modes, block);
    CFRunLoopWakeUp(main);
}

static NSString *const StatusIdentifier = @"voiceport.item45.status";

static id axValue(AXUIElementRef element, CFStringRef key) {
    CFTypeRef value = NULL;
    if (AXUIElementCopyAttributeValue(element, key, &value) != kAXErrorSuccess) return nil;
    return CFBridgingRelease(value);
}

// Stay within this process's AX tree. AX is consulted from a worker queue so a
// synchronous request cannot block the app's main thread servicing AppKit.
static BOOL axMatches(NSString *phase, NSString *title) {
    traceEvent(phase.UTF8String, "ax-start");
    AXUIElementRef app = AXUIElementCreateApplication(getpid());
    AXUIElementSetMessagingTimeout(app, 0.15f);
    CFTypeRef root = NULL;
    AXError statusError = AXUIElementCopyAttributeValue(app, kAXExtrasMenuBarAttribute, &root);
    CFRelease(app);
    if (statusError != kAXErrorSuccess || !root) {
        if (root) CFRelease(root);
        const char *reason = "ax-status-root-unavailable";
        if (statusError == kAXErrorAPIDisabled) reason = "ax-api-disabled";
        if (statusError == kAXErrorCannotComplete) reason = "ax-root-messaging-failed";
        traceEvent(phase.UTF8String, reason);
        return NO;
    }
    id extras = CFBridgingRelease(root);
    BOOL foundStatus = NO;
    BOOL foundMenu = NO;
    for (id entry in axValue((__bridge AXUIElementRef)extras, kAXChildrenAttribute)) {
        AXUIElementRef status = (__bridge AXUIElementRef)entry;
        if (![axValue(status, kAXIdentifierAttribute) isEqual:StatusIdentifier] &&
            ![axValue(status, kAXTitleAttribute) isEqual:StatusIdentifier] &&
            ![axValue(status, kAXDescriptionAttribute) isEqual:StatusIdentifier]) continue;
        foundStatus = YES;
        for (id submenu in axValue(status, kAXChildrenAttribute)) {
            AXUIElementRef menu = (__bridge AXUIElementRef)submenu;
            if (![axValue(menu, kAXRoleAttribute) isEqual:(__bridge NSString *)kAXMenuRole]) continue;
            foundMenu = YES;
            for (id row in axValue(menu, kAXChildrenAttribute)) {
                AXUIElementRef item = (__bridge AXUIElementRef)row;
                if ([axValue(item, kAXRoleAttribute) isEqual:(__bridge NSString *)kAXMenuItemRole] &&
                    [axValue(item, kAXTitleAttribute) isEqual:title] &&
                    [axValue(item, kAXEnabledAttribute) isEqual:@NO]) {
                    traceEvent(phase.UTF8String, "ax-matched");
                    return YES;
                }
            }
        }
    }
    traceEvent(phase.UTF8String, !foundStatus ? "ax-status-item-unavailable" :
               !foundMenu ? "ax-menu-unavailable" : "ax-disabled-title-unverified");
    return NO;
}

static NSString *normalize(NSString *text) {
    NSString *value = [text precomposedStringWithCompatibilityMapping].lowercaseString;
    return [[value componentsSeparatedByCharactersInSet:
        [NSCharacterSet alphanumericCharacterSet].invertedSet] componentsJoinedByString:@""];
}

static BOOL pixelsMatch(CGImageRef image, NSString *phase, NSString *expected) {
    traceEvent(phase.UTF8String, "ocr-start");
    VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
    request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
    request.recognitionLanguages = @[@"zh-Hans", @"en-US"];
    request.usesLanguageCorrection = NO;
    VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
    if (![handler performRequests:@[request] error:NULL]) {
        traceEvent(phase.UTF8String, "ocr-request-failed");
        return NO;
    }
    // Require both the entire expected update row and a fixed sibling row in
    // the actual window. Strip decoration, but separately require percentage
    // and rate punctuation so an incomplete/stale row cannot pass.
    BOOL sibling = NO;
    BOOL row = NO;
    NSString *wanted = normalize(expected);
    for (VNRecognizedTextObservation *observation in request.results) {
        NSString *text = [observation topCandidates:1].firstObject.string;
        NSString *normalized = normalize(text);
        sibling |= [normalized isEqualToString:@"自动检查更新"];
        if (![normalized isEqualToString:wanted]) continue;
        NSString *compat = [text precomposedStringWithCompatibilityMapping];
        row |= [compat containsString:@"%"] &&
            [compat.lowercaseString containsString:@"/s"] == [expected containsString:@"/s"];
    }
    traceEvent(phase.UTF8String, !sibling ? "ocr-sibling-unverified" :
               !row ? "ocr-title-unverified" : "ocr-matched");
    return sibling && row;
}

int item45_attach(NSMenu *menu, NSStatusBarButton *button, const char *output, int trace) {
    if (![NSThread isMainThread] || menu == nil || button.window == nil || trackingGeneration != 0) return 0;
    // Native callbacks may arrive until process exit, after Rust drops its file copy.
    traceFD = trace >= 0 ? dup(trace) : -1;
    statusWindow = button.window.windowNumber;
    [button setAccessibilityLabel:StatusIdentifier];
    outputDirectory = [NSURL fileURLWithFileSystemRepresentation:output isDirectory:YES relativeToURL:nil];
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    [center addObserverForName:NSMenuDidBeginTrackingNotification object:menu queue:nil usingBlock:^(__unused NSNotification *note) {
        if (tracking) return;
        tracking = YES;
        trackingGeneration += 1;
        trackedWindow = 0;
        traceEvent(trackingPhase(), "begin");
        printf("tracking begin generation=%ld\n", (long)trackingGeneration);
    }];
    [center addObserverForName:NSMenuDidEndTrackingNotification object:menu queue:nil usingBlock:^(__unused NSNotification *note) {
        if (!tracking) return;
        tracking = NO;
        traceEvent(trackingPhase(), "end");
        printf("tracking end generation=%ld\n", (long)trackingGeneration);
    }];
    return 1;
}

int item45_tracking(void) {
    return (int)(tracking ? trackingGeneration : -trackingGeneration);
}

int item45_sample_result(void) { return (int)sampleResult; }

void item45_expire_sample(void) { ++sampleToken; }

int item45_sample(const char *label, const char *title) {
    if (!tracking) return 0;
    const NSInteger generation = trackingGeneration;
    const NSInteger token = ++sampleToken;
    sampleResult = 0;
    NSString *phase = [NSString stringWithUTF8String:label];
    NSString *expected = [NSString stringWithUTF8String:title];
    traceEvent(label, "sample-start");
    void (^done)(const char *, BOOL, NSData *) = ^(const char *reason, BOOL ax, NSData *frame) {
        traceEvent(phase.UTF8String, reason);
        onMainDuringTracking(^{
            if (sampleToken != token) return;
            BOOL stayedOpen = tracking && trackingGeneration == generation;
            BOOL pixels = NO;
            if (stayedOpen && frame) {
                NSURL *target = [outputDirectory URLByAppendingPathComponent:[phase stringByAppendingPathExtension:@"png"]];
                pixels = [frame writeToURL:target atomically:YES];
                traceEvent(phase.UTF8String, pixels ? "frame-written" : "frame-write-failed");
            }
            traceEvent(phase.UTF8String, !stayedOpen ? "sample-ended" :
                       ax && pixels ? "sample-verified" : "sample-unverified");
            NSString *stage = pixels ? @"sampled" : @"unverified";
            printf("presentation phase=%s generation=%ld window=%u tracking=%d ax=%d pixels=%d stage=%s\n",
                   phase.UTF8String, (long)generation, trackedWindow, stayedOpen, ax, pixels, stage.UTF8String);
            sampleResult = stayedOpen && ax && pixels ? 1 : -1;
        });
    };
    // Current-process enumeration is the public API that does not ask TCC for
    // other applications. Never use display capture, a picker, or a permission request.
    [SCShareableContent getCurrentProcessShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        traceEvent(phase.UTF8String, "own-content-replied");
        onMainDuringTracking(^{
            if (sampleToken != token || !tracking || generation != trackingGeneration) {
                traceEvent(phase.UTF8String, "content-arrived-after-sample");
                return;
            }
            if (error || !content) { done("own-content-unavailable", NO, nil); return; }
            SCWindow *window = nil;
            for (SCWindow *candidate in content.windows) {
                if (candidate.owningApplication.processID != getpid() || !candidate.isOnScreen ||
                    candidate.windowID == statusWindow || candidate.frame.size.width < 100 ||
                    candidate.frame.size.height < 70 ||
                    candidate.windowLayer != CGWindowLevelForKey(kCGPopUpMenuWindowLevelKey)) continue;
                if (window) { done("ambiguous-own-menu-window", NO, nil); return; }
                window = candidate;
            }
            if (!window) {
                traceEvent(phase.UTF8String, "own-menu-window-missing");
                // Still sample the owned AX menu if WindowServer's menu surface
                // is not exposed as a captureable window of this app.
                dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                    done("own-menu-window-unavailable", axMatches(phase, expected), nil);
                });
                return;
            }
            if (trackedWindow && trackedWindow != window.windowID) {
                done("window-identity-changed", NO, nil);
                return;
            }
            trackedWindow = window.windowID;
            traceEvent(phase.UTF8String, "capture-requested");
            SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:window];
            SCStreamConfiguration *config = [SCStreamConfiguration new];
            config.width = (size_t)ceil(window.frame.size.width * 2);
            config.height = (size_t)ceil(window.frame.size.height * 2);
            config.showsCursor = NO;
            config.ignoreShadowsSingleWindow = YES;
            [SCScreenshotManager captureImageWithFilter:filter configuration:config completionHandler:^(CGImageRef image, NSError *captureError) {
                traceEvent(phase.UTF8String, "capture-replied");
                if (!image || captureError) {
                    traceEvent(phase.UTF8String, "own-menu-capture-unavailable");
                    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                        done("capture-check-complete", axMatches(phase, expected), nil);
                    });
                    return;
                }
                CGImageRetain(image);
                dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                    BOOL ax = axMatches(phase, expected);
                    NSData *frame = nil;
                    if (pixelsMatch(image, phase, expected)) {
                        NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:image];
                        frame = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
                        if (!frame) traceEvent(phase.UTF8String, "frame-encode-failed");
                    }
                    CGImageRelease(image);
                    done("capture-check-complete", ax, frame);
                });
            }];
        });
    }];
    return 1;
}
