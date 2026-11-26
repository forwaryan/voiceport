#import <AppKit/AppKit.h>
#import <CoreGraphics/CGSession.h>
#import <Security/AuthSession.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static const char *flag(CFDictionaryRef session, CFStringRef key) {
    CFTypeRef value = CFDictionaryGetValue(session, key);
    if (!value) return "missing";
    if (CFGetTypeID(value) != CFBooleanGetTypeID()) return "invalid";
    return CFBooleanGetValue(value) ? "true" : "false";
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        uid_t uid = geteuid();
        struct stat console;
        SecuritySessionId asid = 0;
        SessionAttributeBits attributes = 0;
        OSStatus status = SessionGetInfo(callerSecuritySession, &asid, &attributes);
        if (!uid || getuid() != uid || stat("/dev/console", &console) != 0 ||
            console.st_uid != uid || status != errSecSuccess || !asid ||
            (attributes & (sessionHasGraphicAccess | sessionIsRemote)) != sessionHasGraphicAccess) {
            fprintf(stderr, "item23 caller is outside a non-root local graphical console session\n");
            return 78;
        }
        CFDictionaryRef session = CGSessionCopyCurrentDictionary();
        if (!session) return 78;
        CFTypeRef user = CFDictionaryGetValue(session, kCGSessionUserIDKey);
        int64_t quartzUID = -1;
        if (user && CFGetTypeID(user) == CFNumberGetTypeID()) {
            CFNumberGetValue(user, kCFNumberSInt64Type, &quartzUID);
        }
        const char *onConsole = flag(session, kCGSessionOnConsoleKey);
        const char *loginDone = flag(session, kCGSessionLoginDoneKey);
        const char *locked = flag(session, CFSTR("CGSSessionScreenIsLocked"));
        // An unlocked session may omit the lock flag. Also require an
        // active, regular desktop application and exclude lock/login UI.
        NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
        NSString *bundle = front.bundleIdentifier;
        NSArray *systemUI = @[@"com.apple.loginwindow", @"com.apple.SecurityAgent",
                              @"com.apple.ScreenSaver.Engine", @"com.apple.screensaver"];
        BOOL describe = argc == 3 && !strcmp(argv[1], "--describe-handoff");
        BOOL active = front.active;
        BOOL regular = (active || describe) && front != nil &&
                       front.activationPolicy == NSApplicationActivationPolicyRegular;
        BOOL named = ((active && regular) || describe) && bundle.length != 0;
        BOOL excluded = named && [systemUI containsObject:bundle];
        BOOL desktop = active && regular && named && !excluded;
        fprintf(stderr, "item23 uid_matches=%d graphics=1 remote=0 sdk_on_console=%s "
                        "login_done=%s screen_lock=%s desktop_frontmost=%d\n",
                quartzUID == uid, onConsole, loginDone, locked, desktop);
        BOOL sessionEligible = quartzUID == uid && !strcmp(onConsole, "true") &&
                               !strcmp(loginDone, "true") &&
                               (!strcmp(locked, "false") || !strcmp(locked, "missing"));
        BOOL eligible = sessionEligible && desktop;
        if (describe) {
            char callerASID[16];
            snprintf(callerASID, sizeof(callerASID), "%u", asid);
            fprintf(stderr, "item23 handoff present=%d active=%d regular=%d named=%d excluded=%d "
                            "session_eligible=%d caller_asid_matches=%d\n",
                    front != nil, active, regular, named, excluded,
                    sessionEligible, !strcmp(callerASID, argv[2]));
        }
        CFRelease(session);
        if (!eligible) return 78;
        printf("%u %u\n", uid, asid);
        return 0;
    }
}
