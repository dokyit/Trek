//! The iPhone tool's Apps and Files beside its screen, as on a PC (`windows.rs`): the apps of
//! the user's own (installed over usbmuxd, as Finder does), a .ipa to install, and the media
//! folder or an app's Documents to browse, copy into, save from and remove from. Nothing here
//! needs Xcode, Developer Mode or an app on the phone.

use super::PhonePanel;
use crate::assets::Lucide;
use crate::fluent;
use crate::scale::{radius, text, to_rems};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use trek_phone::device::{self, App as PhoneApp};
use trek_phone::files::{self, Entry};

/// What the tool shows under its strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    /// The phone's screen, a click on it a tap.
    #[default]
    Screen,
    Apps,
    Files,
}

/// A folder the Files list shows: in the media folder, or in an app's Documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Place {
    app: Option<String>,
    path: String,
}

impl Default for Place {
    fn default() -> Self {
        Place::media()
    }
}

impl Place {
    fn media() -> Self {
        Place {
            app: None,
            path: "/".into(),
        }
    }

    fn documents(bundle: String) -> Self {
        Place {
            app: Some(bundle),
            path: "/Documents".into(),
        }
    }

    /// Its top: the media folder's root, or the app's Documents.
    fn top(&self) -> &'static str {
        if self.app.is_some() {
            "/Documents"
        } else {
            "/"
        }
    }

    /// The folder above, kept inside the app's Documents.
    fn parent(&self) -> Option<Place> {
        if self.path == self.top() {
            return None;
        }
        let parent = files::parents(&self.path)
            .pop()
            .unwrap_or_else(|| "/".to_string());
        let path = if self.app.is_some() && parent == "/" {
            "/Documents".to_string()
        } else {
            parent
        };
        Some(Place {
            app: self.app.clone(),
            path,
        })
    }

    /// Files can be copied in: a folder of the media folder (not its top, nor the Camera Roll,
    /// which is Photos' own), or an app's Documents.
    fn takes_files(&self) -> bool {
        self.app.is_some()
            || (self.path != "/"
                && files::check_remote(&files::join(&self.path, "x"), false).is_ok())
    }
}

/// The Apps and Files lists, and what the tool is doing with them for the user.
#[derive(Default)]
pub(super) struct Lists {
    pub(super) tab: Tab,
    /// The phone's apps, with the phone they were read from.
    apps: Option<(String, Result<Vec<PhoneApp>, String>)>,
    apps_loading: bool,
    place: Place,
    /// The folder last listed, and what was in it.
    entries: Option<(Place, Result<Vec<Entry>, String>)>,
    entries_loading: bool,
    /// The file whose Remove was clicked once: the next click removes it.
    removing: Option<String>,
    /// What the tool is doing for the user ("Installing Foo…").
    working: Option<String>,
}

fn shown_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

impl PhonePanel {
    fn udid(&self) -> Option<String> {
        self.phone.as_ref().map(|p| p.udid.clone())
    }

    /// Run blocking work off the UI thread, then apply its result (no window needed).
    fn in_background<R: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| done(this, result, cx));
        })
        .detach();
    }

    /// A list comes into view: read again (an agent may have installed or copied meanwhile).
    pub(super) fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.lists.tab = tab;
        // Another phone since: its own lists.
        let udid = self.udid();
        if self.lists.apps.as_ref().map(|(u, _)| u) != udid.as_ref() {
            self.lists.apps = None;
        }
        match tab {
            Tab::Apps => self.load_apps(cx),
            Tab::Files => {
                self.load_entries(cx);
                // The apps say which take files (the place menu).
                if self.lists.apps.is_none() {
                    self.load_apps(cx);
                }
            }
            Tab::Screen => {}
        }
        cx.notify();
    }

    fn load_apps(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.udid() else {
            return;
        };
        if self.lists.apps_loading || self.demo {
            return;
        }
        self.lists.apps_loading = true;
        let for_udid = udid.clone();
        self.in_background(
            cx,
            move || device::apps(Some(&udid)),
            move |this, apps, cx| {
                this.lists.apps_loading = false;
                this.lists.apps = Some((for_udid, apps));
                cx.notify();
            },
        );
        cx.notify();
    }

    fn load_entries(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.udid() else {
            return;
        };
        if self.demo {
            return;
        }
        self.lists.entries_loading = true;
        self.lists.removing = None;
        let place = self.lists.place.clone();
        let listed = place.clone();
        self.in_background(
            cx,
            move || device::list_dir(Some(&udid), &place.path, place.app.as_deref()),
            move |this, entries, cx| {
                // Only what's still asked for: the user may have moved on meanwhile.
                if this.lists.place == listed {
                    this.lists.entries_loading = false;
                    this.lists.entries = Some((listed, entries));
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn go_to(&mut self, place: Place, cx: &mut Context<Self>) {
        self.lists.place = place;
        self.load_entries(cx);
    }

    fn open_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        let place = Place {
            app: self.lists.place.app.clone(),
            path: files::join(&self.lists.place.path, name),
        };
        self.go_to(place, cx);
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        if let Some(place) = self.lists.place.parent() {
            self.go_to(place, cx);
        }
    }

    /// Something the user did with the lists ended: its outcome in the notice line.
    fn finished(&mut self, outcome: String, cx: &mut Context<Self>) {
        self.lists.working = None;
        self.notice = Some(outcome.into());
        cx.notify();
    }

    /// The user's own install: a .ipa picked on this Mac (or dropped on the list).
    fn install_app(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Install".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            if let Some(package) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| this.install_package(package, cx));
            }
        })
        .detach();
    }

    fn install_package(&mut self, package: PathBuf, cx: &mut Context<Self>) {
        let Some(udid) = self.udid() else {
            return;
        };
        let name = shown_name(&package);
        if !package
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ipa"))
        {
            self.notice =
                Some(format!("{name} isn't an app: pick a .ipa signed for this iPhone.").into());
            cx.notify();
            return;
        }
        self.lists.working = Some(format!("Installing {name}…"));
        self.notice = None;
        self.in_background(
            cx,
            move || device::install(Some(&udid), &package),
            move |this, done, cx| {
                this.finished(
                    match done {
                        Ok(()) => format!("Installed {name}. If it's development-signed, it opens once its developer is trusted on the iPhone (Settings › General › VPN & Device Management)."),
                        Err(e) => format!("Couldn't install {name}: {e}"),
                    },
                    cx,
                );
                this.lists.apps = None;
                this.load_apps(cx);
            },
        );
        cx.notify();
    }

    /// The user's own copy onto the phone, into the folder the Files list shows.
    fn add_file(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Copy to iPhone".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            if let Some(local) = paths.into_iter().next() {
                let _ = this.update(cx, |this, cx| this.copy_in(local, cx));
            }
        })
        .detach();
    }

    fn copy_in(&mut self, local: PathBuf, cx: &mut Context<Self>) {
        let Some(udid) = self.udid() else {
            return;
        };
        let place = self.lists.place.clone();
        let name = shown_name(&local);
        let remote = files::join(&place.path, &name);
        self.lists.working = Some(format!("Copying {name} to the iPhone…"));
        self.notice = None;
        self.in_background(
            cx,
            move || device::push(Some(&udid), &local, &remote, place.app.as_deref()),
            move |this, done, cx| {
                this.finished(
                    match done {
                        Ok(n) => format!("Copied {name} ({}) to the iPhone.", files::human_size(n)),
                        Err(e) => format!("Couldn't copy {name}: {e}"),
                    },
                    cx,
                );
                this.load_entries(cx);
            },
        );
        cx.notify();
    }

    /// The user's own copy off the phone, to where they choose on this Mac.
    fn save_file(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(udid) = self.udid() else {
            return;
        };
        let home = trek_core::paths::home();
        let downloads = home.join("Downloads");
        let dir = if downloads.is_dir() { downloads } else { home };
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        let place = self.lists.place.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(local))) = rx.await else {
                return;
            };
            let remote = files::join(&place.path, &name);
            let shown = local.display().to_string();
            let _ = this.update(cx, |this, cx| {
                this.lists.working = Some(format!("Saving {name}…"));
                this.notice = None;
                this.in_background(
                    cx,
                    move || device::pull(Some(&udid), &remote, &local, place.app.as_deref()),
                    move |this, done, cx| {
                        this.finished(
                            match done {
                                Ok(_) => format!("Saved {name} to {shown}."),
                                Err(e) => format!("Couldn't save {name}: {e}"),
                            },
                            cx,
                        )
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// Remove a file: the first click asks, the second removes it.
    fn remove_file(&mut self, name: String, cx: &mut Context<Self>) {
        if self.lists.removing.as_deref() != Some(name.as_str()) {
            self.lists.removing = Some(name);
            cx.notify();
            return;
        }
        self.lists.removing = None;
        let Some(udid) = self.udid() else {
            return;
        };
        let place = self.lists.place.clone();
        let remote = files::join(&place.path, &name);
        self.lists.working = Some(format!("Removing {name}…"));
        self.in_background(
            cx,
            move || device::remove(Some(&udid), &remote, place.app.as_deref()),
            move |this, done, cx| {
                this.finished(
                    match done {
                        Ok(()) => format!("Removed {name} from the iPhone."),
                        Err(e) => format!("Couldn't remove {name}: {e}"),
                    },
                    cx,
                );
                this.load_entries(cx);
            },
        );
        cx.notify();
    }

    /// A file dropped on the Apps list (a .ipa) or the Files list (copied into its folder).
    pub(super) fn dropped(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let Some(path) = paths.paths().first().cloned() else {
            return;
        };
        match self.lists.tab {
            Tab::Apps => self.install_package(path, cx),
            Tab::Files if self.lists.place.takes_files() => self.copy_in(path, cx),
            Tab::Files => {
                self.notice = Some(
                    "Open a folder to copy into first (Downloads, say; not the Camera Roll)."
                        .into(),
                );
                cx.notify();
            }
            Tab::Screen => {}
        }
    }

    /// Screen, Apps and Files, with the list's own action at the end.
    pub(super) fn tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let count = match &self.lists.apps {
            Some((_, Ok(apps))) => format!("Apps · {}", apps.len()),
            _ => "Apps".to_string(),
        };
        let panel = cx.entity().downgrade();
        let pick = move |tab: Tab, _: &mut Window, cx: &mut App| {
            let _ = panel.update(cx, |p, cx| p.show_tab(tab, cx));
        };
        let busy = self.lists.working.is_some();
        let can_add = self.lists.place.takes_files();
        let action = match self.lists.tab {
            Tab::Screen => div().into_any_element(),
            Tab::Apps => Button::new("phone-install")
                .xsmall()
                .outline()
                .icon(Lucide::PackagePlus)
                .label("Install an app…")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.install_app(cx)))
                .into_any_element(),
            Tab::Files => Button::new("phone-add-file")
                .xsmall()
                .outline()
                .icon(Lucide::FilePlus)
                .label("Add a file…")
                .disabled(busy || !can_add)
                .tooltip(if can_add {
                    "Copy a file from this Mac into this folder"
                } else {
                    "Open a folder to copy into first (Downloads, say; not the Camera Roll)"
                })
                .on_click(cx.listener(|this, _, _, cx| this.add_file(cx)))
                .into_any_element(),
        };
        v_flex()
            .child(
                h_flex()
                    .px_3()
                    .py(px(6.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(crate::ui::segmented(
                        "phone-tab",
                        vec![
                            (Tab::Screen, "Screen".to_string()),
                            (Tab::Apps, count),
                            (Tab::Files, "Files".to_string()),
                        ],
                        self.lists.tab,
                        pick,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(action),
            )
            .children(self.lists.working.clone().map(|what| {
                h_flex()
                    .id("phone-working")
                    .test_support()
                    .px_3()
                    .py(px(6.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(to_rems(text::META))
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().xsmall())
                    .child(what)
            }))
            .into_any_element()
    }

    pub(super) fn apps_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list = v_flex()
            .id("phone-apps")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1();
        let list = match &self.lists.apps {
            None => list.items_center().pt_6().child(Spinner::new().small()),
            Some((_, Err(e))) => list
                .px_3()
                .pt_2()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(format!("Couldn't list the apps: {e}")),
            Some((_, Ok(apps))) if apps.is_empty() => list
                .px_3()
                .pt_2()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child("No apps of yours on this iPhone yet. Install one signed for it with Install an app…, or drop a .ipa here."),
            Some((_, Ok(apps))) => list.children(apps.iter().map(|a| self.app_row(a, cx))),
        };
        list.drag_over::<ExternalPaths>(|s, _, _, cx| s.bg(cx.theme().foreground.opacity(0.04)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.dropped(paths, cx)))
            .into_any_element()
    }

    fn app_row(&self, a: &PhoneApp, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let docs = a.bundle.clone();
        h_flex()
            .id(SharedString::from(format!("phone-app-{}", a.bundle)))
            .test_support()
            .mx_1()
            .h(px(40.))
            .px_2()
            .gap_2()
            .rounded(px(fluent::control(radius::CONTROL)))
            .hover(|s| s.bg(theme.list_hover))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(26.))
                    .rounded(px(radius::CONTROL))
                    .bg(theme.foreground.opacity(0.08))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(Lucide::AppWindow)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(to_rems(text::LABEL))
                            .truncate()
                            .child(a.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(if a.version.is_empty() {
                                a.bundle.clone()
                            } else {
                                format!("{} · {}", a.version, a.bundle)
                            }),
                    ),
            )
            .when(a.file_sharing, |el| {
                el.child(
                    Button::new(SharedString::from(format!("phone-app-files-{}", a.bundle)))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Folder)
                        .label("Files")
                        .tooltip("This app takes files: open its Documents")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.lists.tab = Tab::Files;
                            this.go_to(Place::documents(docs.clone()), cx);
                        })),
                )
            })
    }

    /// Where the Files list is: the media folder or an app's Documents (a menu), the folder's
    /// path, and the way up.
    fn place_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let sharing: Vec<(String, String)> = match &self.lists.apps {
            Some((_, Ok(apps))) => apps
                .iter()
                .filter(|a| a.file_sharing)
                .map(|a| (a.bundle.clone(), a.name.clone()))
                .collect(),
            _ => vec![],
        };
        let label = match &self.lists.place.app {
            None => "iPhone".to_string(),
            Some(bundle) => sharing
                .iter()
                .find(|(b, _)| b == bundle)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| bundle.clone()),
        };
        let chosen = self.lists.place.app.clone();
        let panel = cx.entity().downgrade();
        let places = Button::new("phone-files-place")
            .xsmall()
            .ghost()
            .icon(IconName::ChevronDown)
            .label(label)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                let go = |place: Place| {
                    let panel = panel.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let place = place.clone();
                        let _ = panel.update(cx, |p, cx| p.go_to(place, cx));
                    }
                };
                menu = menu.min_w(px(220.)).item(
                    PopupMenuItem::new("iPhone (its media folder)")
                        .icon(Lucide::Smartphone)
                        .checked(chosen.is_none())
                        .on_click(go(Place::media())),
                );
                for (bundle, name) in &sharing {
                    menu = menu.item(
                        PopupMenuItem::new(format!("{name}'s Documents"))
                            .icon(IconName::Folder)
                            .checked(chosen.as_deref() == Some(bundle.as_str()))
                            .on_click(go(Place::documents(bundle.clone()))),
                    );
                }
                menu
            });
        let at_top = self.lists.place.parent().is_none();
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .text_size(to_rems(text::META))
            .border_b_1()
            .border_color(theme.border)
            .child(places)
            .child(
                crate::ui::icon_button("phone-files-up", IconName::ArrowUp, "Up a folder")
                    .xsmall()
                    .disabled(at_top)
                    .on_click(cx.listener(|this, _, _, cx| this.up(cx))),
            )
            .child(
                div()
                    .id("phone-files-path")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(self.lists.place.path.clone()),
            )
            .into_any_element()
    }

    pub(super) fn files_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list = v_flex()
            .id("phone-files")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1();
        let here = self
            .lists
            .entries
            .as_ref()
            .filter(|(place, _)| *place == self.lists.place && !self.lists.entries_loading);
        let list = match here {
            None => list.items_center().pt_6().child(Spinner::new().small()),
            Some((_, Err(e))) => list
                .px_3()
                .pt_2()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(format!("Couldn't list {}: {e}", self.lists.place.path)),
            Some((_, Ok(entries))) if entries.is_empty() => list
                .px_3()
                .pt_2()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child("Nothing here yet."),
            Some((_, Ok(entries))) => {
                let place = &self.lists.place;
                let camera_roll = place.app.is_none()
                    && files::check_remote(&files::join(&place.path, "x"), false).is_err();
                list.children(entries.iter().map(|e| self.file_row(e, camera_roll, cx)))
            }
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .child(self.place_bar(cx))
            .child(
                list.drag_over::<ExternalPaths>(|s, _, _, cx| {
                    s.bg(cx.theme().foreground.opacity(0.04))
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.dropped(paths, cx))),
            )
            .into_any_element()
    }

    /// One file or folder: a folder opens; a file is saved to this Mac, or removed (not in the
    /// Camera Roll, which is Photos' own).
    fn file_row(&self, e: &Entry, camera_roll: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let name = e.name.clone();
        let row = h_flex()
            .id(SharedString::from(format!("phone-file-{}", e.name)))
            .test_support()
            .mx_1()
            .h(px(28.))
            .px_2()
            .gap(px(6.))
            .rounded(px(fluent::control(radius::CONTROL)))
            .text_size(to_rems(text::LABEL))
            .hover(|s| s.bg(theme.list_hover))
            .child(if e.dir {
                crate::file_icon::folder(&e.name, false, px(14.), cx)
            } else {
                crate::file_icon::badge(&e.name, px(14.), cx)
            })
            .child(div().flex_1().min_w_0().truncate().child(e.name.clone()));
        if e.dir {
            return row
                .cursor_pointer()
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_folder(&name, cx)))
                .into_any_element();
        }
        let asking = self.lists.removing.as_deref() == Some(e.name.as_str());
        let busy = self.lists.working.is_some();
        let (save, remove) = (name.clone(), name);
        row.child(
            div()
                .text_size(to_rems(text::META))
                .text_color(theme.muted_foreground)
                .child(files::human_size(e.size)),
        )
        .child(
            crate::ui::icon_button(
                SharedString::from(format!("phone-file-save-{}", e.name)),
                Lucide::Download,
                "Save to this Mac…",
            )
            .xsmall()
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.save_file(save.clone(), cx))),
        )
        .when(!camera_roll, |row| {
            row.child(
                Button::new(SharedString::from(format!("phone-file-remove-{}", e.name)))
                    .xsmall()
                    .when(asking, |b| b.danger().label("Remove?"))
                    .when(!asking, |b| b.ghost().icon(Lucide::Trash))
                    .tooltip(if asking {
                        "Click again to remove it from the iPhone"
                    } else {
                        "Remove from the iPhone"
                    })
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.remove_file(remove.clone(), cx)),
                    ),
            )
        })
        .into_any_element()
    }

    /// Made-up lists for design review (`control phone apps` / `files`).
    #[cfg(feature = "shots")]
    pub(super) fn demo_lists(&mut self, tab: Tab) {
        self.lists = Lists {
            tab,
            apps: Some((
                "demo".into(),
                Ok(vec![
                    PhoneApp {
                        bundle: "dev.trek.TrekMobile".into(),
                        name: "Trek".into(),
                        version: "0.4.54".into(),
                        build: "1290".into(),
                        file_sharing: true,
                    },
                    PhoneApp {
                        bundle: "com.example.notes".into(),
                        name: "Field Notes".into(),
                        version: "2.1".into(),
                        build: "41".into(),
                        file_sharing: false,
                    },
                ]),
            )),
            place: Place {
                app: None,
                path: "/Downloads".into(),
            },
            entries: Some((
                Place {
                    app: None,
                    path: "/Downloads".into(),
                },
                Ok(vec![
                    Entry {
                        name: "Receipts".into(),
                        dir: true,
                        size: 0,
                    },
                    Entry {
                        name: "trip-plan.pdf".into(),
                        dir: false,
                        size: 284_000,
                    },
                ]),
            )),
            ..Lists::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::Place;

    #[test]
    fn files_go_up_within_their_place_and_in_only_where_they_can() {
        let media = Place::media();
        assert_eq!(media.parent(), None, "the media folder's top");
        assert!(!media.takes_files(), "not into the media folder's top");
        let downloads = Place {
            app: None,
            path: "/Downloads/Trips".into(),
        };
        assert!(downloads.takes_files());
        assert_eq!(downloads.parent().unwrap().path, "/Downloads");
        let docs = Place::documents("dev.trek.TrekMobile".into());
        assert_eq!(docs.parent(), None, "an app's Documents is its top");
        assert!(docs.takes_files());
        let inside = Place {
            app: Some("dev.trek.TrekMobile".into()),
            path: "/Documents/Exports".into(),
        };
        assert_eq!(inside.parent().unwrap(), docs);
    }
}
