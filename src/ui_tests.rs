//! End-to-end tests that drive the real editor view with simulated mouse and
//! keyboard input through gpui's headless test platform.

use gpui::{
    Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, point, prelude::*, px,
    size,
};

use crate::editor::{CsvEditor, EditTarget, SelKind};
use crate::grid::{HEADER_H, ROW_H};
use crate::parse::parse_str;
use crate::theme::Theme;

const CSV: &str = "name,age,city\nann,30,Oslo\nbob,25,Rome\ncat,41,Oslo\ndan,19,Paris\n";

fn setup(cx: &mut TestAppContext, csv: &str) -> (Entity<CsvEditor>, VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        crate::actions::bind_keys(cx);
        crate::text_input::bind_keys(cx);
    });
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1000.), px(600.)),
                ))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| CsvEditor::new(window, cx)),
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let editor = window.root(&mut vcx).unwrap();
    let parsed = parse_str(csv, b',');
    editor.update_in(&mut vcx, |editor, window, cx| {
        editor.load_parsed(parsed, None, None);
        for col in &mut editor.doc.as_mut().unwrap().columns {
            col.width = 100.;
        }
        window.focus(&editor.focus, cx);
        cx.notify();
    });
    vcx.run_until_parked();
    (editor, vcx)
}

/// Window position of the middle of a cell, or of a column header when `row` is None.
fn cell_pos(
    editor: &Entity<CsvEditor>,
    cx: &mut VisualTestContext,
    row: Option<usize>,
    col: usize,
) -> Point<Pixels> {
    editor.read_with(cx, |e, _| {
        let geo = e.geometry(e.grid_bounds);
        let x = geo.col_screen_x(col) + 40.;
        let y = match row {
            Some(r) => geo.row_screen_y(r) + ROW_H / 2.,
            None => geo.origin.y + HEADER_H / 2.,
        };
        point(px(x), px(y))
    })
}

fn row_header_pos(
    editor: &Entity<CsvEditor>,
    cx: &mut VisualTestContext,
    row: usize,
) -> Point<Pixels> {
    editor.read_with(cx, |e, _| {
        let geo = e.geometry(e.grid_bounds);
        point(
            px(geo.origin.x + 10.),
            px(geo.row_screen_y(row) + ROW_H / 2.),
        )
    })
}

fn column(editor: &Entity<CsvEditor>, cx: &mut VisualTestContext, col: usize) -> Vec<String> {
    editor.read_with(cx, |e, _| {
        let doc = e.doc.as_ref().unwrap();
        (0..doc.row_count())
            .map(|r| doc.cell(r, col).to_string())
            .collect()
    })
}

fn headers(editor: &Entity<CsvEditor>, cx: &mut VisualTestContext) -> Vec<String> {
    editor.read_with(cx, |e, _| {
        e.doc
            .as_ref()
            .unwrap()
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect()
    })
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    let m = Modifiers::default();
    cx.simulate_mouse_down(from, MouseButton::Left, m);
    let steps = 5;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        cx.simulate_event(MouseMoveEvent {
            position: p,
            modifiers: m,
            pressed_button: Some(MouseButton::Left),
        });
    }
    cx.simulate_mouse_up(to, MouseButton::Left, m);
}

fn double_click(cx: &mut VisualTestContext, at: Point<Pixels>) {
    let m = Modifiers::default();
    cx.simulate_click(at, m);
    cx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: m,
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position: at,
        modifiers: m,
        button: MouseButton::Left,
        click_count: 2,
    });
    // Draw a frame so the newly focused editor registers its input handler.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

#[gpui::test]
fn clicking_headers_cycles_sort(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    let age = cell_pos(&editor, &mut cx, None, 1);
    cx.simulate_click(age, Modifiers::default());
    assert_eq!(column(&editor, &mut cx, 1), ["19", "25", "30", "41"]);
    cx.simulate_click(age, Modifiers::default());
    assert_eq!(column(&editor, &mut cx, 1), ["41", "30", "25", "19"]);
    cx.simulate_click(age, Modifiers::default());
    assert_eq!(column(&editor, &mut cx, 1), ["30", "25", "41", "19"]);
}

#[gpui::test]
fn double_click_edits_and_enter_commits(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    let cell = cell_pos(&editor, &mut cx, Some(1), 0);
    double_click(&mut cx, cell);
    let editing = editor.read_with(&cx, |e, _| e.editing.as_ref().map(|e| e.target));
    assert_eq!(editing, Some(EditTarget::Cell(1, 0)));
    cx.simulate_input("!");
    cx.simulate_keystrokes("enter");
    assert_eq!(column(&editor, &mut cx, 0)[1], "bob!");
    let sel = editor.read_with(&cx, |e, _| e.selection.unwrap().anchor);
    assert_eq!(sel, (2, 0), "enter moves down");
    let dirty = editor.read_with(&cx, |e, _| e.is_dirty());
    assert!(dirty);
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(column(&editor, &mut cx, 0)[1], "bob");
}

#[gpui::test]
fn typing_replaces_cell_and_escape_cancels(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    cx.simulate_keystrokes("down right");
    cx.simulate_input("99");
    cx.simulate_keystrokes("tab");
    assert_eq!(column(&editor, &mut cx, 1)[1], "99");
    let sel = editor.read_with(&cx, |e, _| e.selection.unwrap().anchor);
    assert_eq!(sel, (1, 2), "tab moves right");
    cx.simulate_input("x");
    cx.simulate_keystrokes("escape");
    assert_eq!(column(&editor, &mut cx, 2)[1], "Rome");
    let editing = editor.read_with(&cx, |e, _| e.editing.is_some());
    assert!(!editing);
}

#[gpui::test]
fn dragging_a_column_header_moves_the_column(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    let from = cell_pos(&editor, &mut cx, None, 0);
    let to = editor.read_with(&cx, |e, _| {
        let geo = e.geometry(e.grid_bounds);
        point(
            px(geo.col_screen_x(3) - 10.),
            px(geo.origin.y + HEADER_H / 2.),
        )
    });
    drag(&mut cx, from, to);
    assert_eq!(headers(&editor, &mut cx), ["age", "city", "name"]);
    // A drag is not a click, so no sort was applied.
    let sorted = editor.read_with(&cx, |e, _| e.doc.as_ref().unwrap().sort().is_some());
    assert!(!sorted);
}

#[gpui::test]
fn dragging_a_row_header_moves_the_row(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    let from = row_header_pos(&editor, &mut cx, 0);
    let to = row_header_pos(&editor, &mut cx, 2);
    let to = point(to.x, to.y + px(ROW_H / 2. - 2.));
    drag(&mut cx, from, to);
    assert_eq!(column(&editor, &mut cx, 0), ["bob", "cat", "ann", "dan"]);
    let sel = editor.read_with(&cx, |e, _| e.selection.unwrap());
    assert_eq!(sel.kind, SelKind::Rows);
    assert_eq!(sel.anchor.0, 2);
}

#[gpui::test]
fn dragging_a_column_border_resizes(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    let border = editor.read_with(&cx, |e, _| {
        let geo = e.geometry(e.grid_bounds);
        point(px(geo.col_screen_x(1)), px(geo.origin.y + HEADER_H / 2.))
    });
    drag(&mut cx, border, point(border.x + px(60.), border.y));
    let width = editor.read_with(&cx, |e, _| e.doc.as_ref().unwrap().columns[0].width);
    assert!((width - 160.).abs() < 1., "width was {width}");
    let sorted = editor.read_with(&cx, |e, _| e.doc.as_ref().unwrap().sort().is_some());
    assert!(!sorted);
}

#[gpui::test]
fn filter_panel_search_applies_to_matches(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    editor.update_in(&mut cx, |e, window, cx| e.open_filter(2, window, cx));
    cx.run_until_parked();
    cx.simulate_input("osl");
    cx.simulate_keystrokes("enter");
    assert_eq!(column(&editor, &mut cx, 0), ["ann", "cat"]);
    let open = editor.read_with(&cx, |e, _| e.filter.is_some());
    assert!(!open);
}

#[gpui::test]
fn save_writes_only_visible_rows(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    let dir = std::env::temp_dir().join(format!("seesvee-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.csv");
    editor.update_in(&mut cx, |e, window, cx| {
        e.path = Some(path.clone());
        e.open_filter(1, window, cx);
    });
    cx.run_until_parked();
    // Condition: age > 26, via the doc API to keep the test focused on saving.
    editor.update_in(&mut cx, |e, window, cx| {
        let doc = e.doc.as_mut().unwrap();
        doc.set_filter(
            1,
            crate::filter::Filter {
                condition: crate::filter::Condition {
                    op: Some(crate::filter::FilterOp::Greater),
                    value: "26".into(),
                    value2: String::new(),
                },
                values: None,
            },
        );
        doc.toggle_sort(1);
        e.filter = None;
        e.save(window, cx);
    });
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(written, "name,age,city\nann,30,Oslo\ncat,41,Oslo\n");
    let (dirty, rows) = editor.read_with(&cx, |e, _| {
        (e.is_dirty(), e.doc.as_ref().unwrap().total_row_count())
    });
    assert!(!dirty);
    assert_eq!(rows, 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui::test]
fn copy_and_paste_ranges(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, CSV);
    cx.simulate_keystrokes("shift-down shift-right cmd-c");
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied.as_deref(), Some("ann\t30\nbob\t25\n"));
    cx.simulate_keystrokes("down down");
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(column(&editor, &mut cx, 0), ["ann", "bob", "ann", "bob"]);
    assert_eq!(column(&editor, &mut cx, 1), ["30", "25", "30", "25"]);
    cx.simulate_keystrokes("backspace");
    assert_eq!(column(&editor, &mut cx, 0), ["ann", "bob", "", ""]);
}

#[gpui::test]
fn issues_are_reported_with_lines(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, "a,b\n1,2\n3\n4,\"x\n");
    let (count, rows, open) = editor.read_with(&cx, |e, _| {
        (e.issue_count, e.issue_rows.len(), e.issues_open)
    });
    assert_eq!(count, 2);
    assert_eq!(rows, 2);
    assert!(open);
    editor.update(&mut cx, |e, cx| e.jump_to_issue(1, cx));
    let sel = editor.read_with(&cx, |e, _| e.selection.unwrap().anchor);
    assert_eq!(sel, (2, 1));
}

#[gpui::test]
fn header_double_click_on_border_autofits(cx: &mut TestAppContext) {
    let (editor, mut cx) = setup(cx, "n\nshort\na much much longer value here\n");
    let border = editor.read_with(&cx, |e, _| {
        let geo = e.geometry(e.grid_bounds);
        point(px(geo.col_screen_x(1)), px(geo.origin.y + HEADER_H / 2.))
    });
    double_click(&mut cx, border);
    let width = editor.read_with(&cx, |e, _| e.doc.as_ref().unwrap().columns[0].width);
    assert!(width > 200., "width was {width}");
}

#[gpui::test]
fn right_click_menus_run_actions(cx: &mut TestAppContext) {
    use crate::editor::{MenuAction, MenuEntry};
    let (editor, mut cx) = setup(cx, CSV);
    let header = cell_pos(&editor, &mut cx, None, 1);
    cx.simulate_mouse_down(header, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(header, MouseButton::Right, Modifiers::default());
    let labels: Vec<String> = editor.read_with(&cx, |e, _| {
        e.menu
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item { label, .. } => Some(label.to_string()),
                MenuEntry::Separator => None,
            })
            .collect()
    });
    assert!(labels.contains(&"Delete Column".to_string()));
    assert!(labels.contains(&"Filter\u{2026}".to_string()));
    // Click the "Delete Column" item where it was drawn.
    let delete_ix = editor.read_with(&cx, |e, _| {
        e.menu
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    MenuEntry::Item {
                        action: MenuAction::DeleteCols(_),
                        ..
                    }
                )
            })
            .unwrap()
    });
    editor.update_in(&mut cx, |e, window, cx| {
        let action = match &e.menu.as_ref().unwrap().entries[delete_ix] {
            MenuEntry::Item { action, .. } => action.clone(),
            MenuEntry::Separator => unreachable!(),
        };
        e.run_menu_action(action, window, cx);
    });
    assert_eq!(headers(&editor, &mut cx), ["name", "city"]);

    let row = row_header_pos(&editor, &mut cx, 1);
    cx.simulate_mouse_down(row, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(row, MouseButton::Right, Modifiers::default());
    editor.update_in(&mut cx, |e, window, cx| {
        e.run_menu_action(MenuAction::InsertRowAbove(1), window, cx);
        e.run_menu_action(MenuAction::DeleteRows(3..4), window, cx);
    });
    assert_eq!(column(&editor, &mut cx, 0), ["ann", "", "bob", "dan"]);
}
