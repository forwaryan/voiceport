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

static void observePixels(NSData *frame, NSString *phase, NSString *expected) {
    traceEvent(phase.UTF8String, "ocr-start");
    VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
    request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
    request.recognitionLanguages = @[@"zh-Hans", @"en-US"];
    request.usesLanguageCorrection = NO;
    VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithData:frame options:@{}];
    if (![handler performRequests:@[request] error:NULL]) {
        traceEvent(phase.UTF8String, "ocr-request-failed");
        return;
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
    if (!sibling) traceEvent(phase.UTF8String, "ocr-sibling-unverified");
    if (!row) traceEvent(phase.UTF8String, "ocr-title-unverified");
    if (sibling && row) traceEvent(phase.UTF8String, "ocr-matched");
}

static SCWindow *uniqueOwnedPopup(SCShareableContent *content, NSInteger excluded,
                                  const char **failure) {
    SCWindow *window = nil;
    for (SCWindow *candidate in content.windows) {
        if (candidate.owningApplication.processID != getpid() || !candidate.isOnScreen ||
            !candidate.windowID || candidate.windowID == excluded ||
            candidate.frame.size.width < 100 || candidate.frame.size.height < 70 ||
            candidate.windowLayer != CGWindowLevelForKey(kCGPopUpMenuWindowLevelKey)) continue;
        if (window) {
            *failure = "ambiguous-own-menu-window";
            return nil;
        }
        window = candidate;
    }
    if (!window) *failure = "own-menu-window-missing";
    return window;
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
    NSString *phase = [NSString stringWithUTF8String:label];
    NSString *expected = [NSString stringWithUTF8String:title];
    NSDictionary *generations = @{@"rate-50": @1, @"stale-50": @1, @"reopened-75": @2,
                                  @"retry-25": @3, @"verifying-100": @3};
    if (generations[phase] == nil || [generations[phase] integerValue] != generation) return 0;
    const NSInteger token = ++sampleToken;
    const NSInteger excluded = statusWindow;
    sampleResult = 0;
    traceEvent(label, "sample-start");
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        (void)axMatches(phase, expected);
    });
    void (^done)(const char *, CGWindowID, NSData *) = ^(const char *reason, CGWindowID windowID, NSData *frame) {
        traceEvent(phase.UTF8String, reason);
        onMainDuringTracking(^{
            if (sampleToken != token) return;
            if (!tracking || trackingGeneration != generation) {
                traceEvent(phase.UTF8String, "sample-ended");
                sampleResult = -1;
                return;
            }
            if (!frame || !windowID || trackedWindow != windowID) {
                traceEvent(phase.UTF8String, "sample-unverified");
                sampleResult = -1;
                return;
            }
            NSURL *target = [outputDirectory URLByAppendingPathComponent:[phase stringByAppendingPathExtension:@"png"]];
            if (![frame writeToURL:target atomically:YES]) {
                traceEvent(phase.UTF8String, "frame-write-failed");
                sampleResult = -1;
                return;
            }
            traceEvent(phase.UTF8String, "frame-written");
            traceEvent(phase.UTF8String, "sample-window-verified");
            printf("presentation phase=%s generation=%ld window=%u tracking=1 stage=sampled\n",
                   phase.UTF8String, (long)generation, windowID);
            sampleResult = 1;
            dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                observePixels(frame, phase, expected);
            });
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
            if (error || !content) { done("own-content-unavailable", 0, nil); return; }
            const char *failure = NULL;
            SCWindow *window = uniqueOwnedPopup(content, excluded, &failure);
            if (!window) { done(failure, 0, nil); return; }
            const CGWindowID windowID = window.windowID;
            if (trackedWindow && trackedWindow != windowID) {
                done("window-identity-changed", 0, nil);
                return;
            }
            trackedWindow = windowID;
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
                    done("own-menu-capture-unavailable", 0, nil);
                    return;
                }
                CGImageRetain(image);
                // Recheck ownership, uniqueness and identity after capture. The
                // commit on the main thread then checks the same tracking token.
                [SCShareableContent getCurrentProcessShareableContentWithCompletionHandler:^(SCShareableContent *current, NSError *currentError) {
                    const char *failure = "own-content-unavailable";
                    SCWindow *confirmed = currentError || !current ? nil : uniqueOwnedPopup(current, excluded, &failure);
                    if (!confirmed || confirmed.windowID != windowID) {
                        CGImageRelease(image);
                        done(confirmed ? "window-identity-changed" : failure, 0, nil);
                        return;
                    }
                    NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:image];
                    NSData *frame = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
                    CGImageRelease(image);
                    done(frame ? "capture-check-complete" : "frame-encode-failed", windowID, frame);
                }];
            }];
        });
    }];
    return 1;
}
