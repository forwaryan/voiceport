//! Modeless source management. Browsing and saving never run an adapter.
use super::*;
use crate::plugins::{Adapter, FieldKind, Manifest, Source, Store};
mod draft;
mod editor;
mod library;
mod presentation;
pub(super) use library::Library as Manager;

fn alert(d: &Delegate, title: &str, detail: &str, buttons: &[&str]) -> Retained<NSAlert> {
    let a = NSAlert::new(d.mtm());
    a.setMessageText(&NSString::from_str(title));
    a.setInformativeText(&NSString::from_str(detail));
    for b in buttons {
        a.addButtonWithTitle(&NSString::from_str(b));
    }
    a
}

pub(super) fn manage(d: &Delegate) -> Result<(), String> {
    let path = crate::plugins::default_path()?;
    let store = Store::load(&path)?;
    library::show(d, store, path);
    Ok(())
}

fn checkbox(
    d: &Delegate,
    view: &NSView,
    title: &str,
    checked: bool,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::initWithFrame(NSButton::alloc(d.mtm()), frame);
    b.setButtonType(NSButtonType::Switch);
    b.setTitle(&NSString::from_str(title));
    b.setAccessibilityLabel(Some(&NSString::from_str(title)));
    b.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    view.addSubview(&b);
    b
}

/// Keep utility windows within the current display's usable area, including the titlebar.
fn content_size(d: &Delegate, preferred: NSSize) -> NSSize {
    let screen = d
        .ivars()
        .window
        .get()
        .and_then(|w| w.screen())
        .or_else(|| NSScreen::mainScreen(d.mtm()));
    screen.map_or(preferred, |s| {
        let available = s.visibleFrame().size;
        NSSize::new(
            preferred.width.min((available.width - 32.).max(1.)),
            preferred.height.min((available.height - 72.).max(1.)),
        )
    })
}

pub(super) fn confirm_test(d: &Delegate) -> bool {
    let target = d
        .ivars()
        .controller
        .borrow()
        .as_ref()
        .filter(|c| !c.busy())
        .and_then(|c| c.catalog.snapshot(&c.source_id).ok());
    let Some(crate::local_models::SessionSource::Plugin(p)) = target else {
        return false;
    };
    let destination = if let Some(program) = &p.manifest.program {
        format!(
            "本地程序：{}\n参数：{}",
            program.executable.display(),
            serde_json::to_string(&program.args).unwrap_or_default()
        )
    } else {
        format!(
            "服务地址：{}",
            p.source
                .config
                .get("endpoint")
                .map_or("未配置", String::as_str)
        )
    };
    alert(d,"测试转写（使用内置样例）",&format!("来源：{}\n{}\n\n将发送内置一秒静音 WAV 样例及此来源配置。不会启用麦克风或读取输入目标；可使用主界面的取消按钮停止。空 text 表示接口响应但没有识别到语音，本测试不能证明识别质量。",p.source.name,destination),&["取消","发送内置样例"]).runModal()==1001
}
