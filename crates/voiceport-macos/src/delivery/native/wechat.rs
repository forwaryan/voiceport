//! WeChat can omit its focused element. Keep that fallback bound to the main
//! chat window while the shared clipboard backend posts Cmd+V without menus.
use super::*;

fn main_title(title: &str) -> bool {
    matches!(title, "微信" | "WeChat" | "Weixin")
}

fn main_window(role: &str, title: &str, modal: Option<bool>) -> bool {
    role == "AXWindow" && main_title(title) && modal != Some(true)
}

/// Read window metadata only; do not open menus or inspect conversation text.
/// The caller also checks the original window identity and input activity.
pub(super) fn check_window(window: &OwnedCf) -> Result<(), Failure> {
    let role = attribute(window.0, "AXRole")
        .map_err(|_| Failure::Focus)?
        .string(256)
        .ok_or(Failure::Role)?;
    let title = attribute(window.0, "AXTitle")
        .map_err(|_| Failure::Focus)?
        .string(256)
        .ok_or(Failure::Role)?;
    let modal = match attribute(window.0, "AXModal") {
        Ok(value) => unsafe {
            if CFGetTypeID(value.0) != CFBooleanGetTypeID() {
                return Err(Failure::Role);
            }
            Some(CFEqual(value.0, kCFBooleanTrue.cast()) != 0)
        },
        Err(AX_UNSUPPORTED | AX_NO_VALUE) => None,
        Err(_) => return Err(Failure::Focus),
    };
    if !main_window(&role, &title, modal) {
        return Err(Failure::Role);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_focus_fallback_only_accepts_the_nonmodal_main_chat_window() {
        for title in ["微信", "WeChat", "Weixin"] {
            assert!(main_window("AXWindow", title, None));
            assert!(main_window("AXWindow", title, Some(false)));
            assert!(!main_window("AXWindow", title, Some(true)));
            for role in ["AXDialog", "AXSheet", "AXGroup", ""] {
                assert!(!main_window(role, title, None));
            }
        }
        for title in ["视频聊天", "登录", "微信登录", "独立聊天", ""] {
            assert!(!main_window("AXWindow", title, None));
            assert!(!main_window("AXWindow", title, Some(false)));
        }
    }
}
