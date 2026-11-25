use super::*;

impl Delegate {
    pub(in crate::app) fn verify_update_progress_menu(&self) {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let item = NSMenuItem::new(mtm);
        menu.addItem(&item);
        assert!(self.ivars().update_item.set(item.clone()).is_ok());

        let begin = || {
            let (sender, receiver) = mpsc::channel();
            let progress = DownloadProgress::default();
            self.ivars().update.borrow_mut().state = State::Downloading {
                receiver,
                failure: None,
                progress: progress.clone(),
                next_refresh: Instant::now(),
            };
            self.update_menu();
            assert_eq!(item.title().to_string(), "正在连接下载…");
            assert!(!item.isEnabled());
            (sender, progress)
        };
        let (done, progress) = begin();
        let busy = self.update_snapshot();
        assert_eq!(busy["state"], "downloading");
        assert_eq!(busy["busy"], true);
        assert_eq!(busy.as_object().unwrap().len(), 7);

        progress.set_total(Some(5_000_000));
        progress.received(2_100_000);
        self.update_tick();
        let started = item.title().to_string();
        assert_eq!(started, "下载中 42%");
        menu.removeItem(&item);
        std::thread::sleep(Duration::from_millis(550));
        progress.received(3_250_000);
        menu.addItem(&item);
        self.update_menu();
        let reopened = item.title().to_string();
        assert!(reopened.starts_with("下载中 65% · "));
        assert!(reopened.ends_with("/s"));
        assert_eq!(self.update_snapshot(), busy);
        assert!(!item.isEnabled());

        progress.received(5_000_000);
        progress.verifying();
        std::thread::sleep(DOWNLOAD_REFRESH);
        self.update_tick();
        let verifying = item.title().to_string();
        assert_eq!(verifying, "下载完成 100% · 正在校验并暂存…");
        assert_eq!(self.update_snapshot(), busy);
        assert!(!item.isEnabled());
        done.send(Err("合成夹具校验失败。")).unwrap();
        self.update_tick();
        let failed = item.title().to_string();
        assert!(failed.starts_with("无法准备更新…"), "{failed}");
        assert!(item.isEnabled());
        assert_eq!(self.update_snapshot()["busy"], false);

        let (_done, retry) = begin();
        retry.received(500);
        std::thread::sleep(Duration::from_millis(550));
        retry.received(1_500);
        self.update_tick();
        let unknown = item.title().to_string();
        assert!(unknown.starts_with("下载中 · 1.5 KB · 百分比不可用 · "));
        assert!(unknown.ends_with("/s"));
        assert_eq!(self.update_snapshot(), busy);
        assert!(!item.isEnabled());
        println!(
            "PASS detached AppKit update menu: {started} | {reopened} | {verifying} | {failed} | {unknown}"
        );
    }
}
