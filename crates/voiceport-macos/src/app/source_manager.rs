//! Native dialogs. Import/save only read local files; execution requires a separate explicit action.
use super::*;
use crate::plugins::{Adapter, FieldKind, Manifest, Source, Store};
use std::{collections::BTreeMap, path::Path};
mod draft;
mod library;
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
fn popup(d: &Delegate, rows: &[(String, String)]) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(d.mtm()),
        rect(0., 0., 480., 30.),
        false,
    );
    picker::set_rows(&p, rows);
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
    let store = Store::load(&path)?;
    library::show(d, store, path);
    Ok(())
}
fn remove(d: &Delegate, store: &mut Store, path: &Path, id: &str) -> Result<(), String> {
    let source = store
        .sources
        .iter()
        .find(|s| s.id == id)
        .ok_or("来源不存在。")?;
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
        != 1001
    {
        return Ok(());
    }
    let result = crate::source_service::remove(store, path, id);
    d.with_controller(|c| c.refresh_catalog());
    result
}
fn import(d: &Delegate, store: &mut Store, path: &Path) -> Result<Option<String>, String> {
    let panel = NSOpenPanel::openPanel(d.mtm());
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setTitle(Some(ns_string!("选择 VoicePort 插件清单")));
    panel.setPrompt(Some(ns_string!("审阅插件")));
    panel.setMessage(Some(ns_string!(
        "选择插件作者提供的 .json 清单；适配器程序须已安装在本机。下一步将展示插件和程序信息，确认后才导入。不会下载或运行程序。"
    )));
    if panel.runModal() != 1 {
        return Ok(None);
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
        return Ok(None);
    }
    let id = manifest.id.clone();
    store.import(manifest)?;
    save(d, store, path)?;
    Ok(Some(id))
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
    let rows: Vec<_> = plugins
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                format!("{} · {} · 停止后转写", p.name, p.version),
            )
        })
        .collect();
    let choice = popup(d, &rows);
    if let Some(plugin) = old.and_then(|s| plugins.iter().find(|p| p.id == s.plugin_id)) {
        picker::select_id(&choice, &plugin.id);
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
    let id = picker::selected_id(&choice)?;
    plugins.into_iter().find(|p| p.id == id)
}
fn edit(d: &Delegate, store: &mut Store, path: &Path, index: Option<usize>) -> Result<(), String> {
    let old = index.and_then(|i| store.sources.get(i)).cloned();
    let Some(plugin) = old
        .as_ref()
        .and_then(|s| store.plugin(&s.plugin_id).ok())
        .or_else(|| choose_plugin(d, store, old.as_ref()))
    else {
        return Ok(());
    };
    edit_with_plugin(d, store, path, index, plugin)
}
fn edit_with_plugin(
    d: &Delegate,
    store: &mut Store,
    path: &Path,
    index: Option<usize>,
    plugin: Manifest,
) -> Result<(), String> {
    let old = index.and_then(|i| store.sources.get(i)).cloned();
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
    name.setPlaceholderString(Some(ns_string!("例如：我的转写服务")));
    let mut fields = vec![];
    for (i, f) in plugin.fields.iter().enumerate() {
        let y = height - 102. - i as f64 * 54.;
        let value = retained
            .and_then(|s| s.config.get(&f.id))
            .map_or("", String::as_str);
        let field_label = if plugin.adapter == Adapter::Http {
            match f.id.as_str() {
                "endpoint" => "完整转写接口地址",
                "model" => "模型 ID（按服务商文档填写）",
                "api_key" => "API Key",
                _ => &f.label,
            }
        } else {
            &f.label
        };
        let has_key = retained.is_some_and(|s| s.credential_refs.contains_key(&f.id));
        let label = format!(
            "{}{}{}",
            field_label,
            if f.required { " *" } else { "" },
            if f.kind == FieldKind::Secret && has_key {
                "（留空保留已有密钥）"
            } else if !f.required {
                "（可选）"
            } else {
                ""
            }
        );
        let control = input(d, &view, &label, value, y, f.kind == FieldKind::Secret);
        if plugin.adapter == Adapter::Http {
            let placeholder = match f.id.as_str() {
                "endpoint" => "例如：https://api.example.com/v1/audio/transcriptions",
                "model" => "例如：whisper-1；以服务商提供的模型 ID 为准",
                "api_key" if has_key => "已保存；留空保留，输入新值则替换",
                "api_key" => "服务要求鉴权时填写；不是 ChatGPT 登录密码",
                _ => "",
            };
            control.setPlaceholderString(Some(&NSString::from_str(placeholder)));
        }
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
        "启用此来源（保存后可在听写页选择）",
        old.as_ref().is_some_and(|s| s.enabled),
        rect(0., 0., 300., 26.),
    );
    let detail = format!(
        "插件：{} · {}\n{}\n服务信息保存在本机，密钥独立保存在系统钥匙串。保存不执行插件、不发送音频。",
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
            "需要支持 OpenAI 兼容音频转写的服务。地址须包含 /v1/audio/transcriptions 等完整接口路径，不能只填官网或控制台地址。\n填写服务地址、模型 ID 和所需的 API Key；保存并启用后，回听写页选择此来源。保存不测试连接。远程须为 HTTPS，本机可用 loopback HTTP。".into()
        }
    );
    let title = if old.is_some() {
        "编辑转写来源"
    } else if plugin.adapter == Adapter::Http {
        "连接兼容转写 API"
    } else {
        "配置导入的插件"
    };
    let a = alert(d, title, &detail, &["保存来源", "取消"]);
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(d.mtm()),
        rect(0., 0., 500., height.min(380.)),
    );
    scroll.setHasVerticalScroller(height > 380.);
    scroll.setDocumentView(Some(&view));
    view.scrollPoint(NSPoint::new(0., (height - 380.).max(0.)));
    a.setAccessoryView(Some(&scroll));
    loop {
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
        for (f, control, remove) in &fields {
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
                        .insert(f.id.clone(), format!("key-{}", uuid::Uuid::new_v4()));
                    pending.push((f.id.clone(), value));
                }
            } else if !value.trim().is_empty() {
                source.config.insert(f.id.clone(), value.trim().into());
            }
        }
        if let Err(message) = draft::validate_with_pending(store, index, &source, &pending) {
            a.setInformativeText(&NSString::from_str(&format!(
                "{message}\n请修改后再次保存。已填写的内容仍保留。\n\n{detail}"
            )));
            continue;
        }
        let result = crate::source_service::save_source(store, path, index, source, pending);
        d.with_controller(|c| {
            if result.is_ok() {
                c.message = "来源配置已保存；保存未发送网络请求或执行插件。".into();
            }
            c.refresh_catalog();
        });
        return result;
    }
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
