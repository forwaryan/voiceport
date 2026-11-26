use super::*;

impl Delegate {
    pub(in crate::app) fn verify_update_progress_views(&self, version: &str) {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let item = NSMenuItem::new(mtm);
        menu.addItem(&item);
        assert!(self.ivars().update_item.set(item.clone()).is_ok());
        self.ivars().context_menu_visible.set(true);

        let host = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 480., 360.));
        let (view, key_views) = self.build_update_page(host.bounds());
        host.addSubview(&view);
        let read = |assert_page: &dyn Fn(&page::Page)| {
            let update = self.ivars().update.borrow();
            assert_page(update.page.as_ref().unwrap());
        };
        read(&|page| {
            assert!(page.versions.stringValue().to_string().contains(version));
            assert!(page.download.isEnabled());
            assert!(!page.install.isEnabled());
            let expected: [&NSView; 3] = [&page.check, &page.download, &page.install];
            assert!(
                key_views
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| std::ptr::eq(&**a, b))
            );
        });
        assert!(self.take_staged_for_install().is_none());

        let confirmed = {
            let update = self.ivars().update.borrow();
            let State::Offered(offer) = &update.state else {
                panic!("expected candidate from the update check");
            };
            Arc::clone(&offer.candidate)
        };
        // Run the production attempt/worker transition with a synthetic
        // transfer and staging body.
        let begin = || {
            let (started, start) = mpsc::channel();
            let (done, finish) = mpsc::channel();
            let expected = version.to_owned();
            assert!(
                self.begin_update_download(&confirmed, move |candidate, progress| {
                    assert_eq!(candidate.version, expected);
                    started.send(progress.clone()).unwrap();
                    finish.recv_timeout(Duration::from_secs(8)).unwrap()
                })
            );
            let progress = start.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(item.title().to_string(), "正在连接下载…");
            assert!(item.isEnabled());
            read(&|page| {
                assert_eq!(page.status.stringValue().to_string(), "正在连接下载…");
                assert_eq!(
                    page.metrics.stringValue().to_string(),
                    "已下载 0 B · 总量未知"
                );
                assert!(page.progress.isHidden());
                assert!(!page.install.isEnabled());
            });
            (done, progress)
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
        self.ivars().context_menu_visible.set(false);
        read(&|page| {
            assert_eq!(
                page.metrics.stringValue().to_string(),
                "已下载 2.1 MB · 42%"
            );
            assert_eq!(page.progress.doubleValue(), 42.);
        });
        view.removeFromSuperview();
        std::thread::sleep(Duration::from_millis(550));
        progress.received(3_250_000);
        self.update_tick();
        assert_eq!(item.title().to_string(), started);
        self.ivars().context_menu_visible.set(true);
        self.prepare_context_menu();
        let title = item.title().to_string();
        assert!(title.starts_with("下载中 65% · "));
        assert!(title.ends_with("/s"));
        assert_eq!(self.compact_update_title(), title);
        let (reopened, _) = self.build_update_page(host.bounds());
        assert!(std::ptr::eq(&*reopened, &*view));
        host.addSubview(&reopened);
        read(&|page| {
            let metrics = page.metrics.stringValue().to_string();
            assert!(metrics.starts_with("已下载 3.2 MB · 65% · "), "{metrics}");
            assert!(metrics.ends_with("/s"));
            assert_eq!(page.progress.doubleValue(), 65.);
        });
        assert_eq!(self.update_snapshot(), busy);

        std::thread::sleep(Duration::from_secs(2));
        self.update_tick();
        assert_eq!(item.title().to_string(), "下载中 65%");
        read(&|page| {
            assert_eq!(
                page.metrics.stringValue().to_string(),
                "已下载 3.2 MB · 65%"
            );
            assert_eq!(page.detail.stringValue().to_string(), "等待新的下载数据…");
            assert!(!page.install.isEnabled());
        });
        progress.received(5_000_000);
        progress.verifying();
        std::thread::sleep(DOWNLOAD_REFRESH);
        self.update_tick();
        let verifying = item.title().to_string();
        assert_eq!(verifying, "下载完成 100% · 正在校验并暂存…");
        assert_eq!(self.compact_update_title(), verifying);
        read(&|page| {
            assert_eq!(page.status.stringValue().to_string(), "正在校验并暂存…");
            assert_eq!(page.progress.doubleValue(), 100.);
            assert!(!page.install.isEnabled());
        });
        assert_eq!(self.update_snapshot(), busy);
        assert!(self.take_staged_for_install().is_none());

        let wait_until = |phase| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.update_view_snapshot().phase != phase {
                assert!(Instant::now() < deadline, "did not reach {phase:?}");
                self.update_tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        done.send(Err("合成夹具校验失败。")).unwrap();
        wait_until(UpdateViewPhase::DownloadFailed);
        assert!(item.title().to_string().starts_with("无法准备更新…"));
        assert!(item.isEnabled());
        assert_eq!(self.update_snapshot()["busy"], false);
        read(&|page| {
            assert_eq!(page.detail.stringValue().to_string(), "合成夹具校验失败。");
            assert_eq!(page.download.title().to_string(), "重试下载");
            assert!(page.download.isEnabled());
            assert!(!page.install.isEnabled());
        });
        assert!(self.take_staged_for_install().is_none());

        let (done, retry) = begin();
        retry.received(500);
        std::thread::sleep(Duration::from_millis(550));
        retry.received(1_500);
        self.update_tick();
        let unknown = item.title().to_string();
        assert!(unknown.starts_with("下载中 · 1.5 KB · 百分比不可用 · "));
        assert!(unknown.ends_with("/s"));
        assert_eq!(self.compact_update_title(), unknown);
        read(&|page| {
            let metrics = page.metrics.stringValue().to_string();
            assert!(
                metrics.starts_with("已下载 1.5 KB · 总量未知 · "),
                "{metrics}"
            );
            assert!(metrics.ends_with("/s"));
            assert!(page.progress.isHidden());
            assert!(!page.install.isEnabled());
        });
        assert_eq!(self.update_snapshot(), busy);
        assert!(item.isEnabled());
        retry.verifying();
        std::thread::sleep(DOWNLOAD_REFRESH);
        self.update_tick();
        read(&|page| {
            assert_eq!(page.status.stringValue().to_string(), "正在校验并暂存…");
            assert_eq!(
                page.metrics.stringValue().to_string(),
                "已下载 1.5 KB · 总量未知"
            );
            assert!(page.progress.isHidden());
            assert!(!page.install.isEnabled());
        });
        done.send(Ok(Staged::fixture(version))).unwrap();
        wait_until(UpdateViewPhase::Ready);
        assert_eq!(self.update_snapshot()["state"], "ready_to_install");
        read(&|page| {
            assert_eq!(
                page.status.stringValue().to_string(),
                "更新已校验，可以安装"
            );
            assert!(page.install.isEnabled());
            assert!(!page.download.isEnabled());
        });
        let (staged, _, _) = self
            .take_staged_for_install()
            .expect("staged installation gate");
        assert_eq!(staged.version, version);
        assert!(self.take_staged_for_install().is_none());
        assert!(self.ivars().update_commit.borrow().is_none());
        self.ivars().context_menu_visible.set(false);
        println!(
            "PASS detached AppKit update views: versions, bytes, speed, stall, reopen, unknown total, verifying, failure/retry, and staged-only installation gate"
        );
    }
}
