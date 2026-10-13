mod actions;
mod default_app;
mod document;
mod editor;
mod filter;
mod filter_panel;
mod grid;
mod parse;
mod store;
mod text_input;
mod theme;
#[cfg(test)]
mod ui_tests;
mod value;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, AsyncApp, Bounds, Menu, MenuItem, PathPromptOptions, PromptLevel, SystemMenuType,
    TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;

use actions::*;
use editor::CsvEditor;
use theme::Theme;

pub fn open_window(
    path: Option<PathBuf>,
    blank: bool,
    cx: &mut App,
) -> Option<WindowHandle<CsvEditor>> {
    let offset = px(26. * (cx.windows().len() % 8) as f32);
    let mut bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
    bounds.origin += point(offset, offset);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Seesvee".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(420.), px(280.))),
                focus: true,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| CsvEditor::new(window, cx)),
        )
        .ok()?;
    handle
        .update(cx, |editor, window, cx| {
            window.focus(&editor.focus, cx);
            if let Some(path) = path {
                editor.open_path(path, None, window, cx);
            } else if blank {
                editor.new_blank(cx);
            }
            window.activate_window();
        })
        .ok();
    Some(handle)
}

/// Opens each path in the frontmost empty window, or a new one.
fn open_paths(paths: Vec<PathBuf>, cx: &mut App) {
    for path in paths {
        let empty = cx.windows().into_iter().find_map(|w| {
            let handle = w.downcast::<CsvEditor>()?;
            handle
                .read_with(cx, |editor, _| editor.is_empty())
                .ok()
                .filter(|empty| *empty)
                .map(|_| handle)
        });
        match empty {
            Some(handle) => {
                handle
                    .update(cx, |editor, window, cx| {
                        editor.open_path(path, None, window, cx);
                        window.activate_window();
                    })
                    .ok();
            }
            None => {
                open_window(Some(path), false, cx);
            }
        }
    }
}

fn prompt_open(cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some("Open".into()),
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Ok(Ok(Some(paths))) = rx.await {
            cx.update(|cx| open_paths(paths, cx));
        }
    })
    .detach();
}

fn quit(cx: &mut App) {
    let dirty: Vec<WindowHandle<CsvEditor>> = cx
        .windows()
        .into_iter()
        .filter_map(|w| w.downcast::<CsvEditor>())
        .filter(|h| h.read_with(cx, |e, _| e.is_dirty()).unwrap_or(false))
        .collect();
    let Some(first) = dirty.first().copied() else {
        cx.quit();
        return;
    };
    let count = dirty.len();
    first
        .update(cx, |_, window, cx| {
            window.activate_window();
            let message = if count == 1 {
                "You have unsaved changes. Quit anyway?".to_string()
            } else {
                format!("You have unsaved changes in {count} windows. Quit anyway?")
            };
            let rx = window.prompt(
                PromptLevel::Warning,
                &message,
                Some("Your changes will be lost."),
                &["Quit", "Cancel"],
                cx,
            );
            cx.spawn(async move |_, cx| {
                if rx.await == Ok(0) {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        })
        .ok();
}

fn make_default_app(cx: &mut App) {
    let (level, message, detail) = match default_app::app_bundle() {
        Some(bundle) => match default_app::make_default(&bundle) {
            Ok(()) if default_app::is_default() => (
                PromptLevel::Info,
                "Seesvee now opens CSV files".to_string(),
                "Double-click any .csv file in Finder to open it here.".to_string(),
            ),
            Ok(()) => (
                PromptLevel::Warning,
                "macOS didn't accept the change".to_string(),
                "Choose Seesvee in a CSV file's Get Info window under \u{201c}Open with\u{201d}, then click Change All.".to_string(),
            ),
            Err(code) => (
                PromptLevel::Warning,
                "Couldn't make Seesvee the default".to_string(),
                format!("LaunchServices returned error {code}."),
            ),
        },
        None => (
            PromptLevel::Warning,
            "Run Seesvee from its app bundle".to_string(),
            "Build it with scripts/bundle-macos.sh and open Seesvee.app.".to_string(),
        ),
    };
    let window = cx.active_window().or_else(|| cx.windows().first().copied());
    if let Some(window) = window {
        window
            .update(cx, |_, window, cx| {
                drop(window.prompt(level, &message, Some(&detail), &["OK"], cx));
            })
            .ok();
    }
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("Seesvee").items([
            MenuItem::action("Make Default App for CSV Files", MakeDefaultApp),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Seesvee", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New", NewFile),
            MenuItem::action("Open\u{2026}", Open),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save As\u{2026}", SaveAs),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Reopen With Delimiter").items([
                MenuItem::action("Comma", ReloadComma),
                MenuItem::action("Semicolon", ReloadSemicolon),
                MenuItem::action("Tab", ReloadTab),
                MenuItem::action("Pipe", ReloadPipe),
            ])),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", Undo),
            MenuItem::action("Redo", Redo),
            MenuItem::separator(),
            MenuItem::action("Cut", Cut),
            MenuItem::action("Copy", Copy),
            MenuItem::action("Paste", Paste),
            MenuItem::action("Clear Contents", ClearCells),
            MenuItem::action("Select All", SelectAll),
            MenuItem::separator(),
            MenuItem::action("Insert Row Above", InsertRowAbove),
            MenuItem::action("Insert Row Below", InsertRowBelow),
            MenuItem::action("Delete Rows", DeleteRows),
            MenuItem::separator(),
            MenuItem::action("Insert Column Left", InsertColumnLeft),
            MenuItem::action("Insert Column Right", InsertColumnRight),
            MenuItem::action("Delete Columns", DeleteColumns),
        ]),
        Menu::new("Data").items([
            MenuItem::action("Filter Column\u{2026}", FilterColumn),
            MenuItem::action("Clear All Filters", ClearAllFilters),
            MenuItem::action("Clear Sort", ClearSort),
            MenuItem::separator(),
            MenuItem::action("Show Problems", ToggleProblems),
        ]),
        Menu::new("Window").items([MenuItem::action("Minimize", Minimize)]),
    ]);
}

fn paths_from_urls(urls: Vec<String>) -> Vec<PathBuf> {
    urls.into_iter()
        .filter_map(|url| url.strip_prefix("file://").map(percent_decode))
        .map(PathBuf::from)
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let app = application();

    // Files opened from Finder arrive as URLs, possibly before launch finishes.
    let pending: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let async_cx: Rc<RefCell<Option<AsyncApp>>> = Rc::default();
    app.on_open_urls({
        let pending = pending.clone();
        let async_cx = async_cx.clone();
        move |urls| {
            let paths = paths_from_urls(urls);
            match async_cx.borrow().as_ref() {
                Some(cx) => {
                    cx.update(|cx| open_paths(paths, cx));
                }
                None => pending.borrow_mut().extend(paths),
            }
        }
    });

    app.run(move |cx: &mut App| {
        cx.set_global(Theme::for_appearance(cx.window_appearance()));
        actions::bind_keys(cx);
        text_input::bind_keys(cx);
        set_menus(cx);

        cx.on_action(|_: &Open, cx| prompt_open(cx));
        cx.on_action(|_: &NewFile, cx| {
            open_window(None, true, cx);
        });
        cx.on_action(|_: &Quit, cx| quit(cx));
        cx.on_action(|_: &MakeDefaultApp, cx| make_default_app(cx));
        default_app::register_on_first_launch();
        cx.on_action(|_: &Minimize, cx| {
            if let Some(window) = cx.active_window() {
                window
                    .update(cx, |_, window, _| window.minimize_window())
                    .ok();
            }
        });
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        *async_cx.borrow_mut() = Some(cx.to_async());
        let mut paths = args.clone();
        paths.extend(pending.borrow_mut().drain(..));
        if paths.is_empty() {
            open_window(None, false, cx);
        } else {
            open_paths(paths, cx);
        }
        cx.activate(true);
    });
}
