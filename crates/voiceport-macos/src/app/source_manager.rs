//! Native dialogs. Import/save only read local files; execution requires a separate explicit action.
use super::*;
use crate::{
    plugin_credentials::Keychain,
    plugins::{Adapter, FieldKind, Manifest, Source, Store},
};
use std::{collections::BTreeMap, path::Path};
fn alert(d: &Delegate, title: &str, detail: &str, buttons: &[&str]) -> Retained<NSAlert> {
    let a = NSAlert::new(d.mtm());
    a.setMessageText(&NSString::from_str(title));
    a.setInformativeText(&NSString::from_str(detail));
    for b in buttons {
        a.addButtonWithTitle(&NSString::from_str(b));
    }
    a
}
fn error(d: &Delegate, message: &str) {
    alert(d, "来源配置未完成", message, &["好"]).runModal();
}
fn popup(d: &Delegate, labels: &[String]) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(d.mtm()),
        rect(0., 0., 480., 30.),
        false,
    );
    for label in labels {
        p.addItemWithTitle(&NSString::from_str(label));
    }
    p
}
fn save(d: &Delegate, store: &Store, path: &Path) -> Result<(), String> {
    store.save(path)?;
    d.with_controller(|c| {
        c.message = "来源配置已保存；保存未发送网络请求或执行插件。".into();
        c.refresh_catalog();
    });
    Ok(())
}
pub(super) fn manage(d: &Delegate) {
    if let Err(e) = manage_result(d) {
        error(d, &e);
    }
}
fn manage_result(d: &Delegate) -> Result<(), String> {
    let path = crate::plugins::default_path()?;
    let mut store = Store::load(&path)?;
    let a = alert(
        d,
        "管理转写来源",
        "一个插件是一种接入方式；可为同一插件添加多个独立来源。ChatGPT 与已检测的 Orca 模型仍在主选择框中。保存、扫描和导入均不发送音频。",
        &[
            "添加来源…",
            "编辑来源…",
            "删除来源…",
            "导入插件清单…",
            "关闭",
        ],
    );
    let labels: Vec<_> = store
        .sources
        .iter()
        .map(|s| {
            format!(
                "{} · {}{}",
                s.name,
                s.plugin_id,
                if s.enabled { "" } else { "（未启用）" }
            )
        })
        .collect();
    let choice = popup(d, &labels);
    choice.setAccessibilityLabel(Some(ns_string!("要管理的自定义来源")));
    a.setAccessoryView(Some(&choice));
    match a.runModal() {
        1000 => edit(d, &mut store, &path, None)?,
        1001 => {
            let i = usize::try_from(choice.indexOfSelectedItem())
                .map_err(|_| "请先添加或选择来源。")?;
            edit(d, &mut store, &path, Some(i))?;
        }
        1002 => {
            let i = usize::try_from(choice.indexOfSelectedItem()).map_err(|_| "请先选择来源。")?;
            let source = store.sources.get(i).ok_or("来源不存在。")?.clone();
            if alert(
                d,
                "删除此来源？",
                &format!(
                    "将删除「{}」及其独立密钥。当前选择若指向它将保持不可用，不会切换云端。",
                    source.name
                ),
                &["取消", "删除"],
            )
            .runModal()
                == 1001
            {
                store.sources.remove(i);
                save(d, &store, &path)?;
                for reference in source.credential_refs.values() {
                    Keychain.delete(reference)?;
                }
            }
        }
        1003 => import(d, &mut store, &path)?,
        _ => {}
    }
    Ok(())
}
fn import(d: &Delegate, store: &mut Store, path: &Path) -> Result<(), String> {
    let panel = NSOpenPanel::openPanel(d.mtm());
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setMessage(Some(ns_string!(
        "选择本机 JSON 插件清单；不会安装或下载程序。"
    )));
    if panel.runModal() != 1 {
        return Ok(());
    }
    let file = panel
        .URL()
        .and_then(|u| u.path())
        .ok_or("未选择本地清单文件。")?;
    let manifest = crate::plugins::read_manifest(Path::new(&file.to_string()))?;
    let program = manifest.program.as_ref().ok_or("插件没有本地程序。")?;
    let detail = format!(
        "清单文件：{}\n插件：{} / {} / {}\n声明来源：{}\n本地程序：{}\n参数数组：{}\n\n插件有当前用户权限，可能读取文件和联网，VoicePort 不提供沙箱。仅在显式启用来源并选中听写或点击样例测试后执行。导入不会执行程序；请仅导入你信任的本地程序。",
        file,
        manifest.name,
        manifest.id,
        manifest.version,
        manifest.origin,
        program.executable.display(),
        serde_json::to_string(&program.args).unwrap_or_default()
    );
    if alert(d, "确认导入外部插件", &detail, &["取消", "导入此清单"]).runModal() != 1001
    {
        return Ok(());
    }
    store.import(manifest)?;
    save(d, store, path)
}
fn input(
    d: &Delegate,
    view: &NSView,
    label: &str,
    value: &str,
    y: f64,
    secret: bool,
) -> Retained<NSTextField> {
    d.label(view, label, rect(0., y + 24., 480., 18.), 12.);
    let field = if secret {
        NSSecureTextField::initWithFrame(NSSecureTextField::alloc(d.mtm()), rect(0., y, 360., 24.))
            .into_super()
    } else {
        NSTextField::initWithFrame(NSTextField::alloc(d.mtm()), rect(0., y, 480., 24.))
    };
    field.setStringValue(&NSString::from_str(value));
    field.setAccessibilityLabel(Some(&NSString::from_str(label)));
    view.addSubview(&field);
    field
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
    b.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    view.addSubview(&b);
    b
}
fn choose_plugin(d: &Delegate, store: &Store, old: Option<&Source>) -> Option<Manifest> {
    let plugins: Vec<_> = store
        .descriptors()
        .into_iter()
        .filter(|p| matches!(p.adapter, Adapter::Http | Adapter::Jsonl))
        .collect();
    let labels: Vec<_> = plugins
        .iter()
        .map(|p| format!("{} · {} · 停止后转写", p.name, p.version))
        .collect();
    let choice = popup(d, &labels);
    if let Some(i) = old.and_then(|s| plugins.iter().position(|p| p.id == s.plugin_id)) {
        choice.selectItemAtIndex(i as isize);
    }
    let a = alert(
        d,
        "选择接入插件",
        "HTTP 插件原生运行，无需 Orca 或 ChatGPT 登录。不同协议可以导入 JSONL 外部适配器。",
        &["继续", "取消"],
    );
    a.setAccessoryView(Some(&choice));
    if a.runModal() != 1000 {
        return None;
    }
    usize::try_from(choice.indexOfSelectedItem())
        .ok()
        .and_then(|i| plugins.get(i).cloned())
}
fn edit(d: &Delegate, store: &mut Store, path: &Path, index: Option<usize>) -> Result<(), String> {
    let old = index.and_then(|i| store.sources.get(i)).cloned();
    let Some(plugin) = choose_plugin(d, store, old.as_ref()) else {
        return Ok(());
    };
    let retained = old.as_ref().filter(|s| s.plugin_id == plugin.id);
    let height = 100. + plugin.fields.len() as f64 * 54.;
    let view = NSView::initWithFrame(NSView::alloc(d.mtm()), rect(0., 0., 480., height));
    let name = input(
        d,
        &view,
        "显示名称",
        old.as_ref().map_or("", |s| s.name.as_str()),
        height - 48.,
        false,
    );
    let mut fields = vec![];
    for (i, f) in plugin.fields.iter().enumerate() {
        let y = height - 102. - i as f64 * 54.;
        let value = retained
            .and_then(|s| s.config.get(&f.id))
            .map_or("", String::as_str);
        let label = format!(
            "{}{}{}",
            f.label,
            if f.required { " *" } else { "" },
            if f.kind == FieldKind::Secret {
                "（留空保留已有密钥）"
            } else {
                ""
            }
        );
        let control = input(d, &view, &label, value, y, f.kind == FieldKind::Secret);
        let remove = if f.kind == FieldKind::Secret {
            Some(checkbox(
                d,
                &view,
                "移除密钥",
                false,
                rect(364., y, 116., 24.),
            ))
        } else {
            None
        };
        fields.push((f, control, remove));
    }
    let enabled = checkbox(
        d,
        &view,
        "显式启用此来源",
        old.as_ref().is_some_and(|s| s.enabled),
        rect(0., 0., 300., 26.),
    );
    let detail = format!(
        "插件：{} · {}\n{}\n配置只保存 Keychain 引用。保存不执行插件、不发送音频；一段录音只使用选中来源。",
        plugin.name,
        plugin.version,
        if let Some(p) = &plugin.program {
            format!(
                "来源：{}\n执行：{} {}",
                plugin.origin,
                p.executable.display(),
                serde_json::to_string(&p.args).unwrap_or_default()
            )
        } else {
            "按完整地址 POST multipart；远程 HTTPS，本机可用 loopback HTTP。".into()
        }
    );
    let a = alert(d, "配置转写来源", &detail, &["保存", "取消"]);
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(d.mtm()),
        rect(0., 0., 500., height.min(380.)),
    );
    scroll.setHasVerticalScroller(height > 380.);
    scroll.setDocumentView(Some(&view));
    view.scrollPoint(NSPoint::new(0., (height - 380.).max(0.)));
    a.setAccessoryView(Some(&scroll));
    if a.runModal() != 1000 {
        return Ok(());
    }
    let mut source = Source {
        id: old.as_ref().map_or_else(
            || format!("source-{}", uuid::Uuid::new_v4()),
            |s| s.id.clone(),
        ),
        name: name.stringValue().to_string().trim().into(),
        plugin_id: plugin.id.clone(),
        plugin_version: plugin.version.clone(),
        enabled: enabled.state() == NSControlStateValueOn,
        config: BTreeMap::new(),
        credential_refs: retained.map_or_else(BTreeMap::new, |s| s.credential_refs.clone()),
    };
    let mut pending = vec![];
    for (f, control, remove) in fields {
        let value = control.stringValue().to_string();
        if f.kind == FieldKind::Secret {
            if remove
                .as_ref()
                .is_some_and(|b| b.state() == NSControlStateValueOn)
            {
                source.credential_refs.remove(&f.id);
            } else if !value.is_empty() {
                source
                    .credential_refs
                    .insert(f.id.clone(), "pending-key-reference".into());
                pending.push((f.id.clone(), value));
            }
        } else if !value.trim().is_empty() {
            source.config.insert(f.id.clone(), value.trim().into());
        }
    }
    // Validate without reading or writing Keychain first; disabled instances can be saved.
    let mut validate = source.clone();
    validate.enabled = true;
    store.validate_source(&validate)?;
    let mut created = vec![];
    let outcome = (|| {
        for (field, value) in pending {
            let reference = Keychain.create(&value)?;
            created.push(reference.clone());
            source.credential_refs.insert(field, reference);
        }
        let mut next = store.clone();
        if let Some(i) = index {
            next.sources[i] = source.clone();
        } else {
            next.sources.push(source.clone());
        }
        save(d, &next, path)?;
        *store = next;
        Ok::<(), String>(())
    })();
    if let Err(e) = outcome {
        for reference in created {
            if Store::reference_is_confirmed_unused(path, &reference) {
                let _ = Keychain.delete(&reference);
            }
        }
        return Err(e);
    }
    if let Some(old) = old {
        for reference in old.credential_refs.values() {
            if !source.credential_refs.values().any(|r| r == reference) {
                Keychain.delete(reference)?;
            }
        }
    }
    Ok(())
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
