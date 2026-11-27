//! Explicit GUI entry points for dictation output and manual presets.
use super::*;
use crate::{
    dictation_dictionary::{format_rules, parse_rules},
    dictation_output::OutputRules,
    dictation_workflows::{Preset, Workflows},
};
use voiceport_core::language;

const APPLY: &str = "应用场景";

pub(super) fn load(controller: &mut Controller, defaults: &NSUserDefaults) {
    let saved = defaults.stringForKey(ns_string!("dictationWorkflowsV1"));
    match Workflows::load(saved.as_ref().map(|value| value.to_string()).as_deref()) {
        Ok(preferences) => controller.workflows = preferences,
        Err(error) => controller.message = error,
    }
}

fn persist(preferences: &Workflows) -> Result<(), String> {
    let encoded = serde_json::to_string(preferences).map_err(|_| "无法保存听写设置。")?;
    unsafe {
        NSUserDefaults::standardUserDefaults().setObject_forKey(
            Some(&NSString::from_str(&encoded)),
            ns_string!("dictationWorkflowsV1"),
        );
    }
    Ok(())
}

impl Delegate {
    pub(super) fn workflow_dialog(&self, action: impl FnOnce() -> Result<(), String>) {
        let iv = self.ivars();
        if self.settings_locked()
            || iv.quitting.get()
            || iv.permission_rx.borrow().is_some()
            || iv.pending_hotkey.borrow().is_some()
            || iv.control.borrow().preparing()
            || iv.controller.borrow().as_ref().is_none_or(|c| c.busy())
        {
            return;
        }
        iv.workflow_dialog.set(true);
        if let Err(error) = action() {
            self.workflow_alert("无法完成操作", &error, &["好"])
                .runModal();
        }
        iv.workflow_dialog.set(false);
        self.refresh();
    }

    pub(super) fn show_dictionary(&self) {
        self.workflow_dialog(|| self.edit_dictionary());
    }

    pub(super) fn show_presets(&self) {
        self.workflow_dialog(|| self.edit_presets());
    }

    pub(super) fn show_original_transcript(&self) {
        self.workflow_dialog(|| {
            let Some(original) = self
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .unwrap()
                .raw_transcript
                .clone()
            else {
                return Ok(());
            };
            let view = self.workflow_view(170.);
            self.workflow_text(&view, &original, "原始转写", 0., 170., false);
            let alert =
                self.workflow_alert("原始转写", "应用文字处理规则前的本次听写结果。", &["关闭"]);
            alert.setAccessoryView(Some(&view));
            alert.runModal();
            Ok(())
        });
    }

    fn edit_dictionary(&self) -> Result<(), String> {
        let mut preferences = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .unwrap()
            .workflows
            .clone();
        let view = self.workflow_view(324.);
        let enable = self.workflow_checkbox(
            &view,
            "对最终听写启用个人词典",
            preferences.dictionary_enabled,
            298.,
        );
        self.label(&view, "纠错与常用短语", rect(0., 273., 410., 22.), 13.);
        let rules = self.workflow_text(
            &view,
            &format_rules(&preferences.rules),
            "词典规则",
            137.,
            130.,
            true,
        );
        let trim = self.workflow_checkbox(
            &view,
            "去除首尾空白",
            preferences.output_rules.trim_edges,
            109.,
        );
        let newlines = self.workflow_checkbox(
            &view,
            "统一换行为 LF",
            preferences.output_rules.normalize_newlines,
            81.,
        );
        self.label(&view, "试一段文字", rect(0., 55., 410., 22.), 13.);
        let sample = self.workflow_text(&view, "", "输出预览输入", 0., 52., true);
        let alert = self.workflow_alert(
            "个人词典与输出",
            "词典每行一条规则，用 → 分隔。例如 北经 → 北京。可在本机预览现有编辑，不会投递或保存样例。",
            &["保存", "预览", "取消"],
        );
        alert.setAccessoryView(Some(&view));
        loop {
            let response = alert.runModal();
            if self.ivars().quitting.get() || response == NSAlertThirdButtonReturn {
                return Ok(());
            }
            if response != NSAlertFirstButtonReturn && response != NSAlertSecondButtonReturn {
                return Ok(());
            }
            let candidate = match parse_rules(&rules.string().to_string()) {
                Ok(candidate) => candidate,
                Err(message) => {
                    self.workflow_alert("检查词典规则", &message, &["返回编辑"])
                        .runModal();
                    continue;
                }
            };
            preferences.rules = candidate;
            preferences.dictionary_enabled = enable.state() == NSControlStateValueOn;
            preferences.output_rules = OutputRules {
                trim_edges: trim.state() == NSControlStateValueOn,
                normalize_newlines: newlines.state() == NSControlStateValueOn,
            };
            if response == NSAlertSecondButtonReturn {
                let sample = sample.string().to_string();
                let pipeline = preferences
                    .output_for_session(preferences.dictionary_enabled, preferences.output_rules);
                let stages = pipeline.process(&sample);
                let preview = self.workflow_view(247.);
                for (label, text, y) in [
                    ("原文", stages.original, 166.),
                    ("词典结果", stages.after_dictionary.as_ref(), 83.),
                    ("最终输出", stages.final_text(), 0.),
                ] {
                    self.label(&preview, label, rect(0., y + 58., 410., 20.), 13.);
                    self.workflow_text(&preview, text, label, y, 56., false);
                }
                let result = self.workflow_alert("输出预览", "", &["返回编辑"]);
                result.setAccessoryView(Some(&preview));
                result.runModal();
                continue;
            }
            self.update_workflows(preferences)?;
            self.with_controller(|c| c.message = "已保存个人词典与输出规则。".into());
            return Ok(());
        }
    }

    fn edit_presets(&self) -> Result<(), String> {
        let preferences = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .unwrap()
            .workflows
            .clone();
        if preferences.presets.is_empty() {
            if self
                .workflow_alert(
                    "听写场景",
                    "保存来源、识别语言、词典开关和当前输出规则，之后可手动应用。",
                    &["新建场景", "关闭"],
                )
                .runModal()
                == NSAlertFirstButtonReturn
            {
                self.create_preset(preferences)?;
            }
            return Ok(());
        }
        let view = self.workflow_view(34.);
        let picker = self.workflow_picker(
            &view,
            preferences
                .presets
                .iter()
                .map(|preset| preset.name.as_str()),
            "已保存的听写场景",
            0.,
            0,
        );
        let alert = self.workflow_alert(
            "听写场景",
            "选择一个场景，确认后将应用到当前听写设置。",
            &[APPLY, "新建场景", "删除场景", "关闭"],
        );
        alert.setAccessoryView(Some(&view));
        let response = alert.runModal();
        if self.ivars().quitting.get() {
            return Ok(());
        }
        if response == NSAlertSecondButtonReturn {
            return self.create_preset(preferences);
        }
        let index = picker.indexOfSelectedItem() as usize;
        let Some(preset) = preferences.presets.get(index) else {
            return Ok(());
        };
        if response == NSAlertFirstButtonReturn {
            self.apply_preset(preset, preferences.clone())?;
        } else if response == NSAlertThirdButtonReturn
            && self
                .workflow_alert("删除听写场景", &preset.name, &["删除", "取消"])
                .runModal()
                == NSAlertFirstButtonReturn
            && !self.ivars().quitting.get()
        {
            let mut preferences = preferences;
            preferences.presets.remove(index);
            self.update_workflows(preferences)?;
        }
        Ok(())
    }

    fn create_preset(&self, mut preferences: Workflows) -> Result<(), String> {
        let (sources, selected_source, selected_language) = {
            let slot = self.ivars().controller.borrow();
            let c = slot.as_ref().unwrap();
            let sources: Vec<_> = c
                .catalog
                .entries
                .iter()
                .filter(|entry| entry.source.is_ok())
                .map(|entry| (entry.id.clone(), entry.title.clone()))
                .collect();
            let selected_source = sources.iter().position(|(id, _)| *id == c.source_id);
            (sources, selected_source, language::index(&c.language))
        };
        if sources.is_empty() {
            return Err("当前没有可用的转写来源。".into());
        }
        let view = self.workflow_view(216.);
        self.label(&view, "场景名称", rect(0., 192., 410., 20.), 13.);
        let name =
            NSTextField::initWithFrame(NSTextField::alloc(self.mtm()), rect(0., 160., 410., 26.));
        name.setAccessibilityLabel(Some(ns_string!("场景名称")));
        view.addSubview(&name);
        self.label(&view, "转写来源", rect(0., 130., 410., 20.), 13.);
        let source = self.workflow_picker(&view, std::iter::empty(), "场景转写来源", 97., 0);
        picker::set_rows(&source, &sources);
        if let Some(index) = selected_source {
            picker::select_id(&source, &sources[index].0);
        }
        self.label(&view, "识别语言", rect(0., 69., 410., 20.), 13.);
        let language = self.workflow_picker(
            &view,
            language::LANGUAGES.iter().map(|(_, title)| *title),
            "场景识别语言",
            38.,
            selected_language,
        );
        let dictionary =
            self.workflow_checkbox(&view, "使用个人词典", preferences.dictionary_enabled, 0.);
        let alert = self.workflow_alert(
            "新建听写场景",
            "新场景会保存当前输出规则，之后可手动应用。",
            &["保存场景", "取消"],
        );
        alert.setAccessoryView(Some(&view));
        loop {
            if alert.runModal() != NSAlertFirstButtonReturn || self.ivars().quitting.get() {
                return Ok(());
            }
            let Some(source_id) = picker::selected_id(&source) else {
                return Err("请选择转写来源。".into());
            };
            let preset = Preset {
                name: name.stringValue().to_string(),
                source_id,
                language: language::code(language.indexOfSelectedItem() as usize).into(),
                dictionary_enabled: dictionary.state() == NSControlStateValueOn,
                output_rules: preferences.output_rules,
            };
            let validation = {
                let slot = self.ivars().controller.borrow();
                preset.source(&slot.as_ref().unwrap().catalog)
            };
            match validation.and_then(|_| preferences.add_preset(preset)) {
                Ok(()) => return self.update_workflows(preferences),
                Err(message) => {
                    self.workflow_alert("检查场景设置", &message, &["返回编辑"])
                        .runModal();
                }
            }
        }
    }

    pub(super) fn confirm_named_scene(&self, name: &str) {
        self.workflow_dialog(|| {
            let preferences = self
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .ok_or("听写尚未就绪。")?
                .workflows
                .clone();
            let preset = preferences
                .presets
                .iter()
                .find(|preset| preset.name == name)
                .cloned()
                .ok_or("场景已不存在。")?;
            self.apply_preset(&preset, preferences)
        });
    }

    fn apply_preset(&self, preset: &Preset, mut preferences: Workflows) -> Result<(), String> {
        let (source, source_title) = {
            let slot = self.ivars().controller.borrow();
            let catalog = &slot.as_ref().unwrap().catalog;
            (
                preset.source(catalog)?,
                catalog
                    .entries
                    .iter()
                    .find(|entry| entry.id == preset.source_id)
                    .unwrap()
                    .title
                    .clone(),
            )
        };
        let destination = source_manager::destination::Destination::for_session(&source);
        let language_title = language::LANGUAGES[language::index(&preset.language)].1;
        let dictionary = if preset.dictionary_enabled {
            "启用"
        } else {
            "关闭"
        };
        let output = [
            (preset.output_rules.trim_edges, "去除首尾空白"),
            (preset.output_rules.normalize_newlines, "统一换行为 LF"),
        ]
        .into_iter()
        .filter_map(|(enabled, title)| enabled.then_some(title))
        .collect::<Vec<_>>()
        .join("、");
        let output = if output.is_empty() { "关闭" } else { &output };
        let details = format!(
            "来源：{source_title}\n识别语言：{language_title}\n个人词典：{dictionary}\n输出规则：{output}\n\n{}",
            destination.details(),
        );
        if self
            .workflow_alert(
                &format!("应用「{}」？", preset.name),
                &details,
                &[APPLY, "取消"],
            )
            .runModal()
            != NSAlertFirstButtonReturn
            || self.ivars().quitting.get()
        {
            return Ok(());
        }
        {
            let slot = self.ivars().controller.borrow();
            if preset.source(&slot.as_ref().unwrap().catalog)? != source {
                return Err("来源配置已变化，请重新确认音频去向。".into());
            }
        }
        preferences.dictionary_enabled = preset.dictionary_enabled;
        preferences.output_rules = preset.output_rules;
        self.update_workflows(preferences)?;
        self.with_controller(|c| {
            self.set_language(c, preset.language.clone());
            self.set_source(c, preset.source_id.clone());
            c.message = format!("已应用听写场景「{}」。", preset.name);
        });
        Ok(())
    }

    fn update_workflows(&self, preferences: Workflows) -> Result<(), String> {
        let mut result = Err("听写尚未就绪。".into());
        self.with_controller(|c| result = self.commit_workflows(c, preferences));
        result
    }

    pub(super) fn commit_workflows(
        &self,
        c: &mut Controller,
        preferences: Workflows,
    ) -> Result<(), String> {
        persist(&preferences)?;
        c.workflows = preferences;
        Ok(())
    }

    pub(super) fn workflow_alert(
        &self,
        title: &str,
        detail: &str,
        buttons: &[&str],
    ) -> Retained<NSAlert> {
        // A choice saved without an alert keeps the quick panel and its focus path.
        self.close_quick_panel();
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(detail));
        for button in buttons {
            let native = alert.addButtonWithTitle(&NSString::from_str(button));
            if matches!(*button, "取消" | "关闭") {
                native.setKeyEquivalent(ns_string!("\u{1b}"));
            }
        }
        alert
    }

    fn workflow_view(&self, height: f64) -> Retained<NSView> {
        NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0., 0., 410., height))
    }

    fn workflow_text(
        &self,
        view: &NSView,
        value: &str,
        label: &str,
        y: f64,
        height: f64,
        editable: bool,
    ) -> Retained<NSTextView> {
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(self.mtm()), rect(0., y, 410., height));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(self.mtm()),
            rect(0., 0., scroll.contentSize().width, height),
        );
        text.setRichText(false);
        text.setAutomaticQuoteSubstitutionEnabled(false);
        text.setAutomaticDashSubstitutionEnabled(false);
        text.setAutomaticTextReplacementEnabled(false);
        text.setAutomaticSpellingCorrectionEnabled(false);
        text.setAutomaticTextCompletionEnabled(false);
        text.setAutomaticLinkDetectionEnabled(false);
        text.setAutomaticDataDetectionEnabled(false);
        text.setContinuousSpellCheckingEnabled(false);
        text.setEditable(editable);
        text.setSelectable(true);
        text.setAccessibilityLabel(Some(&NSString::from_str(label)));
        text.setString(&NSString::from_str(value));
        scroll.setDocumentView(Some(&text));
        view.addSubview(&scroll);
        text
    }

    fn workflow_checkbox(
        &self,
        view: &NSView,
        label: &str,
        checked: bool,
        y: f64,
    ) -> Retained<NSButton> {
        let check = NSButton::initWithFrame(NSButton::alloc(self.mtm()), rect(0., y, 410., 26.));
        check.setButtonType(NSButtonType::Switch);
        check.setTitle(&NSString::from_str(label));
        check.setState(if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        view.addSubview(&check);
        check
    }

    fn workflow_picker<'a>(
        &self,
        view: &NSView,
        titles: impl IntoIterator<Item = &'a str>,
        label: &str,
        y: f64,
        index: usize,
    ) -> Retained<NSPopUpButton> {
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(self.mtm()),
            rect(0., y, 410., 30.),
            false,
        );
        for title in titles {
            popup.addItemWithTitle(&NSString::from_str(title));
        }
        popup.setAccessibilityLabel(Some(&NSString::from_str(label)));
        popup.selectItemAtIndex(index as isize);
        view.addSubview(&popup);
        popup
    }
}
