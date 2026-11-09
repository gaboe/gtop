use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Disclosure {
    Source,
    View,
    Help,
}

#[derive(Default)]
pub(super) struct Controls {
    pub(super) disclosure: Option<Disclosure>,
    menu_index: usize,
    choose_requested: bool,
}

#[derive(Clone, Copy)]
enum Command {
    Key(&'static str),
    Mode(Mode),
    Hidden,
    Apparent,
    Home,
    Volume,
    Choose,
    Menu(Disclosure),
}

fn menu_rows(menu: Disclosure, disk: &Disk) -> Vec<(String, Command)> {
    match menu {
        Disclosure::Source => vec![
            ("Home".into(), Command::Home),
            ("Data volume".into(), Command::Volume),
            ("Choose folder…".into(), Command::Choose),
        ],
        Disclosure::View => vec![
            (
                format!("Fewer levels    [    depth {}", disk.map.draw_depth()),
                Command::Key("["),
            ),
            ("More levels     ]".into(), Command::Key("]")),
            ("Zoom in         +".into(), Command::Key("+")),
            ("Zoom out        −".into(), Command::Key("-")),
            ("Reset zoom      0".into(), Command::Key("0")),
            (
                format!(
                    "Hidden files    {} · rescans",
                    if disk.options.include_hidden {
                        "on"
                    } else {
                        "off"
                    }
                ),
                Command::Hidden,
            ),
            (
                format!(
                    "Apparent size   {} · rescans",
                    if disk.options.apparent_size {
                        "on"
                    } else {
                        "off"
                    }
                ),
                Command::Apparent,
            ),
        ],
        Disclosure::Help => Vec::new(),
    }
}

impl Disk {
    pub(crate) fn take_choose_folder(&mut self) -> bool {
        std::mem::take(&mut self.map.controls.choose_requested)
    }

    fn scan_shelf_status(&self) -> String {
        let state = self
            .session
            .as_ref()
            .and_then(|session| session.outcome.as_ref());
        let label = match state {
            Some(ScanState::Complete) => "Complete",
            Some(ScanState::Cancelled) => "Cancelled · partial sizes",
            Some(ScanState::Failed(_)) => "Failed · partial sizes",
            _ => "Scanning · partial sizes",
        };
        format!(
            "{} · {} observed · {} unreadable",
            label,
            bytes(self.scan_bytes.unwrap_or(0)),
            self.folder.as_ref().map_or(0, |folder| folder.errors)
        )
    }

    pub(super) fn controls_key(&mut self, key: &str) -> bool {
        let toggle = match key {
            "s" => Some(Disclosure::Source),
            "v" => Some(Disclosure::View),
            "?" => Some(Disclosure::Help),
            _ => None,
        };
        if let Some(menu) = toggle {
            self.toggle_controls_menu(menu);
            return true;
        }
        let Some(menu) = self.map.controls.disclosure else {
            return false;
        };
        let rows = menu_rows(menu, self);
        let controls = &mut self.map.controls;
        match key {
            "escape" => controls.disclosure = None,
            "down" | "j" | "tab" => {
                controls.menu_index = (controls.menu_index + 1).min(rows.len().saturating_sub(1))
            }
            "up" | "k" | "shift-tab" => controls.menu_index = controls.menu_index.saturating_sub(1),
            "enter" => {
                if let Some((_, command)) = rows.get(controls.menu_index) {
                    self.run_controls_command(*command);
                }
            }
            _ => {}
        }
        true
    }

    fn toggle_controls_menu(&mut self, menu: Disclosure) {
        let controls = &mut self.map.controls;
        controls.disclosure = if controls.disclosure == Some(menu) {
            None
        } else {
            Some(menu)
        };
        controls.menu_index = 0;
        self.map.siblings = None;
    }

    fn run_controls_command(&mut self, command: Command) {
        if !matches!(command, Command::Menu(_)) {
            self.map.controls.disclosure = None;
        }
        match command {
            Command::Key(key) => match key {
                "d" => self.close(),
                "backspace" => self.up(),
                "c" => self.cancel.store(true, Ordering::Relaxed),
                "r" => self.start_over(),
                _ => {
                    self.map_command(key);
                }
            },
            Command::Mode(mode) => self.set_map_mode(mode),
            Command::Hidden => self.change_scan_option(true),
            Command::Apparent => self.change_scan_option(false),
            Command::Home => self.start(self.home.clone(), false),
            Command::Volume => self.start(disk_scan::system_root(), false),
            Command::Menu(menu) => {
                self.toggle_controls_menu(menu);
                return;
            }
            Command::Choose => self.map.controls.choose_requested = true,
        }
        self.map.controls.disclosure = None;
    }
}

impl Monitor {
    pub(crate) fn controls_choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let request = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Scan folder".into()),
        });
        cx.spawn(async move |entity, cx| {
            if let Ok(Ok(Some(paths))) = request.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = entity.update(cx, |this, cx| {
                        this.disk.as_mut().unwrap().start(path, false);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn controls_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        command: Command,
        cx: &mut Context<Self>,
    ) -> Button {
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        Button::new(id)
            .debug_selector(move || id.into())
            .label(label)
            .rack(&t)
            .small()
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.focus, cx);
                this.disk.as_mut().unwrap().run_controls_command(command);
                if this.disk.as_mut().unwrap().take_choose_folder() {
                    this.controls_choose_folder(window, cx);
                }
                cx.notify();
            }))
    }

    pub(super) fn controls_workspace(
        &self,
        canvas: Stateful<Div>,
        crumbs: Div,
        siblings: Option<Stateful<Div>>,
        legend: Div,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let disk = self.disk.as_ref().unwrap();
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let root = disk
            .session
            .as_ref()
            .map(|session| &session.root)
            .unwrap_or(&disk.home)
            .display()
            .to_string();
        let scanning = disk
            .session
            .as_ref()
            .is_some_and(|session| session.activity == super::ScanActivity::Running);
        let lifecycle = div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(px(8.))
            .child(
                div()
                    .id("controls-scan-status")
                    .role(Role::Status)
                    .text_size(px(10.))
                    .whitespace_nowrap()
                    .text_color(rgb(t.silk_dim))
                    .child(disk.scan_shelf_status()),
            )
            .when(scanning, |row| {
                row.child(self.controls_button("disk-cancel", "Cancel · C", Command::Key("c"), cx))
            })
            .child(self.controls_button("disk-rescan", "Scan again · R", Command::Key("r"), cx));
        let source = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .child(crate::rack::label("SCAN", &t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .truncate()
                    .child(root),
            )
            .child(self.controls_button(
                "controls-source",
                "Change… · S",
                Command::Menu(Disclosure::Source),
                cx,
            ));
        let location = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(self.controls_button("map-back", "Back", Command::Key("b"), cx))
            .child(self.controls_button("map-forward", "Forward", Command::Key("f"), cx))
            .child(self.controls_button("controls-up", "Up", Command::Key("backspace"), cx))
            .child(crumbs);
        let mut modes = div()
            .flex()
            .gap(px(0.))
            .border_1()
            .border_color(rgb(t.rule_hi));
        for (id, label, mode) in [
            ("map-mode-0", "Space", Mode::Size),
            ("map-mode-1", "File count", Mode::Files),
            ("map-mode-2", "Age", Mode::Age),
        ] {
            modes = modes.child(
                self.controls_button(id, label, Command::Mode(mode), cx)
                    .border_0()
                    .bg(rgb(if disk.map.mode == mode {
                        t.selection
                    } else {
                        t.base.plate
                    }))
                    .when(disk.map.mode == mode, |button| {
                        button.text_color(rgb(t.base.led))
                    }),
            );
        }
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(crate::rack::label("MAP BY", &t))
            .child(modes)
            .child(div().flex_1())
            .child(self.controls_button(
                "map-filter",
                if disk.map.filter.is_empty() {
                    "Filter · /".to_string()
                } else {
                    format!("Filter · {}", disk.map.filter)
                },
                Command::Key("/"),
                cx,
            ))
            .child(self.controls_button(
                "controls-view",
                "View · V",
                Command::Menu(Disclosure::View),
                cx,
            ));
        let header = div()
            .debug_selector(|| "controls-navigation".into())
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(6.))
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule))
            .child(location)
            .children(siblings);
        let shelf = div()
            .debug_selector(|| "controls-scan-shelf".into())
            .flex()
            .items_center()
            .gap(px(12.))
            .p(px(8.))
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi))
            .child(source)
            .child(lifecycle);
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(10.))
            .text_color(rgb(t.silk_dim))
            .child(div().flex_1())
            .child(self.controls_button(
                "controls-help",
                "Keys · ?",
                Command::Menu(Disclosure::Help),
                cx,
            ));
        let map = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap(px(8.))
            .child(header)
            .child(toolbar)
            .child(div().text_size(px(10.)).text_color(rgb(t.silk_dim)).child(
                match disk.map.mode {
                    Mode::Files => "Area = file count · empty entries stay in the list",
                    Mode::Age => {
                        "Area = space · colour = newest write · directory strips show kind"
                    }
                    Mode::Size if disk.options.apparent_size => {
                        "Area = apparent size · unknown space has no area"
                    }
                    Mode::Size => "Area = allocated space · unknown space has no area",
                },
            ))
            .child(canvas)
            .child(legend)
            .child(footer);
        let disclosure = disk.map.controls.disclosure;
        let map = if let Some(menu) = disclosure.filter(|menu| *menu != Disclosure::Source) {
            map.child(self.controls_disclosure(menu, cx))
        } else {
            map
        };
        let workspace = div()
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.disk.as_mut().unwrap().map.leave_findings_navigation();
                cx.notify();
            }))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(crate::rack::label("DISK TREEMAP", &t))
                    .child(div().flex_1())
                    .child(self.controls_button(
                        "controls-top",
                        "Back to Top · D",
                        Command::Key("d"),
                        cx,
                    )),
            )
            .child(shelf)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(8.))
                    .child(map)
                    .child(self.resizable_disk_inspector(window, cx)),
            );
        if disclosure == Some(Disclosure::Source) {
            workspace.child(
                self.controls_disclosure(Disclosure::Source, cx)
                    .top(px(84.))
                    .right(px(self
                        .settings
                        .disk_inspector_width
                        .for_window(window.bounds().size.width.into())
                        .pixels())),
            )
        } else {
            workspace
        }
    }

    fn controls_disclosure(&self, menu: Disclosure, cx: &mut Context<Self>) -> Div {
        let disk = self.disk.as_ref().unwrap();
        let controls = &disk.map.controls;
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let mut panel = div()
            .occlude()
            .debug_selector(|| "controls-disclosure".into())
            .absolute()
            .top(px(70.))
            .right(px(0.))
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(12.))
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi));
        panel = panel.child(crate::rack::label(
            match menu {
                Disclosure::Source => "SCAN LOCATION",
                Disclosure::View => "VIEW OPTIONS",
                Disclosure::Help => "KEYBOARD",
            },
            &t,
        ));
        for (index, (label, command)) in menu_rows(menu, disk).into_iter().enumerate() {
            let selector = match command {
                Command::Home => "disk-home",
                Command::Volume => "disk-system",
                Command::Choose => "map-choose-folder",
                Command::Hidden => "map-hidden",
                Command::Apparent => "map-apparent",
                Command::Key("[") => "map-depth-less",
                Command::Key("]") => "map-depth-more",
                Command::Key("+") => "map-zoom-in",
                Command::Key("-") => "map-zoom-out",
                Command::Key("0") => "map-reset",
                _ => unreachable!(),
            };
            let entity = cx.entity();
            panel = panel.child(
                Button::new(("controls-menu-row", index))
                    .debug_selector(move || selector.into())
                    .label(label)
                    .rack(&t)
                    .small()
                    .bg(rgb(if index == controls.menu_index {
                        t.selection
                    } else {
                        t.base.plate
                    }))
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| {
                            window.focus(&this.focus, cx);
                            this.disk.as_mut().unwrap().run_controls_command(command);
                            if this.disk.as_mut().unwrap().take_choose_folder() {
                                this.controls_choose_folder(window, cx);
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        if menu == Disclosure::Help {
            for line in [
                "Arrows / HJKL   Select a tile",
                "W               Findings",
                "↑ ↓ / Enter     Choose / inspect finding",
                "Enter           Open folder",
                "Backspace       Up one folder",
                "Tab             Files list",
                "B / F           Back / Forward",
                "M               Next mode",
                "/               Filter",
                "[ / ]           Fewer / more levels",
                "+ / − / 0       Zoom / reset",
                "X               Review cleanup",
                "C / R           Cancel / scan again",
                "S / V           Scan / View menu",
                "D               Back to Top",
                "Esc             Back out, then Top",
            ] {
                panel = panel.child(div().text_size(px(10.)).child(line));
            }
        }
        panel.child(div().text_size(px(10.)).text_color(rgb(t.silk_dim)).child(
            if menu == Disclosure::Help {
                "Esc closes keyboard help"
            } else {
                "↑ ↓ choose · Enter apply · Esc close"
            },
        ))
    }
}
