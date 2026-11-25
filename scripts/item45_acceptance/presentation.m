#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <Vision/Vision.h>

static NSInteger trackingGeneration;
static BOOL tracking;
static NSInteger sampleToken;
static NSInteger sampleResult;
static CGWindowID trackedWindow;
static NSInteger statusWindow;
static NSURL *outputDirectory;
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
static BOOL axMatches(NSString *title) {
    AXUIElementRef app = AXUIElementCreateApplication(getpid());
    AXUIElementSetMessagingTimeout(app, 0.15f);
    id extras = axValue(app, kAXExtrasMenuBarAttribute);
    CFRelease(app);
    if (extras == nil) return NO;
    for (id entry in axValue((__bridge AXUIElementRef)extras, kAXChildrenAttribute)) {
        AXUIElementRef status = (__bridge AXUIElementRef)entry;
        if (![axValue(status, kAXIdentifierAttribute) isEqual:StatusIdentifier] &&
            ![axValue(status, kAXTitleAttribute) isEqual:StatusIdentifier] &&
            ![axValue(status, kAXDescriptionAttribute) isEqual:StatusIdentifier]) continue;
        for (id submenu in axValue(status, kAXChildrenAttribute)) {
            AXUIElementRef menu = (__bridge AXUIElementRef)submenu;
            if (![axValue(menu, kAXRoleAttribute) isEqual:(__bridge NSString *)kAXMenuRole]) continue;
            for (id row in axValue(menu, kAXChildrenAttribute)) {
                AXUIElementRef item = (__bridge AXUIElementRef)row;
                if ([axValue(item, kAXRoleAttribute) isEqual:(__bridge NSString *)kAXMenuItemRole] &&
                    [axValue(item, kAXTitleAttribute) isEqual:title] &&
                    [axValue(item, kAXEnabledAttribute) isEqual:@NO]) return YES;
            }
        }
    }
    return NO;
}

static NSString *normalize(NSString *text) {
    NSString *value = [text precomposedStringWithCompatibilityMapping].lowercaseString;
    return [[value componentsSeparatedByCharactersInSet:
        [NSCharacterSet alphanumericCharacterSet].invertedSet] componentsJoinedByString:@""];
}

static BOOL pixelsMatch(CGImageRef image, NSString *expected) {
    VNRecognizeTextRequest *request = [VNRecognizeTextRequest new];
    request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
    request.recognitionLanguages = @[@"zh-Hans", @"en-US"];
    request.usesLanguageCorrection = NO;
    VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
    if (![handler performRequests:@[request] error:NULL]) return NO;
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
    return sibling && row;
}

int item45_attach(NSMenu *menu, NSStatusBarButton *button, const char *output) {
    if (![NSThread isMainThread] || menu == nil || button.window == nil || trackingGeneration != 0) return 0;
    statusWindow = button.window.windowNumber;
    [button setAccessibilityLabel:StatusIdentifier];
    outputDirectory = [NSURL fileURLWithFileSystemRepresentation:output isDirectory:YES relativeToURL:nil];
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    [center addObserverForName:NSMenuDidBeginTrackingNotification object:menu queue:nil usingBlock:^(__unused NSNotification *note) {
        if (tracking) return;
        tracking = YES;
        trackingGeneration += 1;
        trackedWindow = 0;
        printf("tracking begin generation=%ld\n", (long)trackingGeneration);
    }];
    [center addObserverForName:NSMenuDidEndTrackingNotification object:menu queue:nil usingBlock:^(__unused NSNotification *note) {
        if (!tracking) return;
        tracking = NO;
        printf("tracking end generation=%ld\n", (long)trackingGeneration);
    }];
    return 1;
}

int item45_tracking(void) {
    return (int)(tracking ? trackingGeneration : -trackingGeneration);
}

int item45_sample_result(void) { return (int)sampleResult; }

int item45_sample(const char *label, const char *title) {
    if (!tracking) return 0;
    const NSInteger generation = trackingGeneration;
    const NSInteger token = ++sampleToken;
    sampleResult = 0;
    NSString *phase = [NSString stringWithUTF8String:label];
    NSString *expected = [NSString stringWithUTF8String:title];
    void (^done)(NSString *, BOOL, BOOL) = ^(NSString *reason, BOOL ax, BOOL pixels) {
        onMainDuringTracking(^{
            if (sampleToken != token) return;
            BOOL stayedOpen = tracking && trackingGeneration == generation;
            printf("presentation phase=%s generation=%ld window=%u tracking=%d ax=%d pixels=%d stage=%s\n",
                   phase.UTF8String, (long)generation, trackedWindow, stayedOpen, ax, pixels, reason.UTF8String);
            sampleResult = stayedOpen && ax && pixels ? 1 : -1;
        });
    };
    // Current-process enumeration is the public API that does not ask TCC for
    // other applications. Never use display capture, a picker, or a permission request.
    [SCShareableContent getCurrentProcessShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        onMainDuringTracking(^{
            if (sampleToken != token || !tracking || generation != trackingGeneration) {
                done(@"tracking-ended", NO, NO);
                return;
            }
            if (error || !content) { done(@"own-content-unavailable", NO, NO); return; }
            SCWindow *window = nil;
            for (SCWindow *candidate in content.windows) {
                if (candidate.owningApplication.processID != getpid() || !candidate.isOnScreen ||
                    candidate.windowID == statusWindow || candidate.frame.size.width < 100 ||
                    candidate.frame.size.height < 70 ||
                    candidate.windowLayer != CGWindowLevelForKey(kCGPopUpMenuWindowLevelKey)) continue;
                if (window) { done(@"ambiguous-own-menu-window", NO, NO); return; }
                window = candidate;
            }
            if (!window) {
                // Still sample the owned AX menu if WindowServer's menu surface
                // is not exposed as a captureable window of this app.
                dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                    done(@"own-menu-window-unavailable", axMatches(expected), NO);
                });
                return;
            }
            if (trackedWindow && trackedWindow != window.windowID) {
                done(@"window-identity-changed", NO, NO);
                return;
            }
            trackedWindow = window.windowID;
            SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:window];
            SCStreamConfiguration *config = [SCStreamConfiguration new];
            config.width = (size_t)ceil(window.frame.size.width * 2);
            config.height = (size_t)ceil(window.frame.size.height * 2);
            config.showsCursor = NO;
            config.ignoreShadowsSingleWindow = YES;
            [SCScreenshotManager captureImageWithFilter:filter configuration:config completionHandler:^(CGImageRef image, NSError *captureError) {
                if (!image || captureError) {
                    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                        done(@"own-menu-capture-unavailable", axMatches(expected), NO);
                    });
                    return;
                }
                CGImageRetain(image);
                dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                    NSURL *target = [outputDirectory URLByAppendingPathComponent:[phase stringByAppendingPathExtension:@"png"]];
                    NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:image];
                    BOOL saved = [[bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToURL:target atomically:YES];
                    BOOL ax = axMatches(expected);
                    BOOL pixels = saved && pixelsMatch(image, expected);
                    CGImageRelease(image);
                    done(saved ? @"sampled" : @"frame-write-failed", ax, pixels);
                });
            }];
        });
    }];
    return 1;
}
