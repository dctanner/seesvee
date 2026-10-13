//! The column filter popover: sort buttons, a condition, and a searchable
//! checklist of the column's values.

use std::collections::HashSet;
use std::ops::Range;

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, UniformListScrollHandle,
    Window, div, prelude::*, px, uniform_list,
};

use crate::document::{Document, SortDir};
use crate::editor::{fmt_int, primary_button, small_button};
use crate::filter::{Condition, Filter, FilterOp, ValueSet};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

const VALUE_LIMIT: usize = 5_000;
const ROW: f32 = 24.0;

pub enum FilterPanelEvent {
    Apply(Filter),
    Sort(SortDir),
    Close,
}

impl EventEmitter<FilterPanelEvent> for FilterPanel {}

pub struct FilterPanel {
    focus: FocusHandle,
    column: String,
    op: Option<FilterOp>,
    op_menu_open: bool,
    value: Entity<TextInput>,
    value2: Entity<TextInput>,
    search: Entity<TextInput>,
    values: Vec<(String, usize)>,
    distinct: usize,
    checked: Vec<bool>,
    visible: Vec<usize>,
    list_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for FilterPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FilterPanel {
    pub fn new(doc: &Document, col: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let existing = doc.filter(col).cloned().unwrap_or_default();
        let (values, distinct) = doc.distinct_values(col, VALUE_LIMIT);
        let checked = values
            .iter()
            .map(|(v, _)| match &existing.values {
                None => true,
                Some(ValueSet::Only(set)) => set.contains(v),
                Some(ValueSet::Except(set)) => !set.contains(v),
            })
            .collect();
        let value = cx.new(|cx| TextInput::new(cx, &existing.condition.value, "Value"));
        let value2 = cx.new(|cx| TextInput::new(cx, &existing.condition.value2, "And"));
        let search = cx.new(|cx| TextInput::new(cx, "", "Search values"));
        let mut subscriptions = Vec::new();
        for input in [&value, &value2] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event, _, cx| match event {
                    TextInputEvent::Submit(_) => this.apply(cx),
                    TextInputEvent::Cancel => cx.emit(FilterPanelEvent::Close),
                    TextInputEvent::Changed => {}
                },
            ));
        }
        subscriptions.push(
            cx.subscribe_in(&search, window, |this, _, event, _, cx| match event {
                TextInputEvent::Changed => this.update_search(cx),
                TextInputEvent::Submit(_) => this.apply(cx),
                TextInputEvent::Cancel => cx.emit(FilterPanelEvent::Close),
            }),
        );
        let focus_target = if existing.condition.op.is_some_and(|op| op.operands() > 0) {
            value.focus_handle(cx)
        } else {
            search.focus_handle(cx)
        };
        window.focus(&focus_target, cx);
        let visible = (0..values.len()).collect();
        Self {
            focus: cx.focus_handle(),
            column: doc.columns[col].name.clone(),
            op: existing.condition.op,
            op_menu_open: false,
            value,
            value2,
            search,
            values,
            distinct,
            checked,
            visible,
            list_scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn update_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().to_lowercase();
        self.visible = (0..self.values.len())
            .filter(|&i| query.is_empty() || self.values[i].0.to_lowercase().contains(&query))
            .collect();
        cx.notify();
    }

    fn all_visible_checked(&self) -> bool {
        self.visible.iter().all(|&i| self.checked[i])
    }

    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let value = !self.all_visible_checked();
        for &i in &self.visible {
            self.checked[i] = value;
        }
        cx.notify();
    }

    fn build(&self, cx: &Context<Self>) -> Filter {
        let condition = Condition {
            op: self.op,
            value: self.value.read(cx).text().to_string(),
            value2: self.value2.read(cx).text().to_string(),
        };
        // While searching, only the checked matches pass, like a spreadsheet autofilter.
        let searching = !self.search.read(cx).text().is_empty();
        let mut effective = self.checked.clone();
        if searching {
            let visible: HashSet<usize> = self.visible.iter().copied().collect();
            for (i, c) in effective.iter_mut().enumerate() {
                *c = *c && visible.contains(&i);
            }
        }
        let unchecked = effective.iter().filter(|c| !**c).count();
        let values = if unchecked == 0 {
            None
        } else {
            let checked = effective.len() - unchecked;
            let complete = self.values.len() == self.distinct;
            let set: HashSet<String> = self
                .values
                .iter()
                .zip(&effective)
                .filter(|(_, c)| **c == (checked <= unchecked && complete))
                .map(|((v, _), _)| v.clone())
                .collect();
            if checked <= unchecked && complete {
                Some(ValueSet::Only(set))
            } else {
                Some(ValueSet::Except(set))
            }
        };
        Filter { condition, values }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let filter = self.build(cx);
        cx.emit(FilterPanelEvent::Apply(filter));
    }

    fn set_op(&mut self, op: Option<FilterOp>, window: &mut Window, cx: &mut Context<Self>) {
        self.op = op;
        self.op_menu_open = false;
        if op.is_some_and(|op| op.operands() > 0) {
            window.focus(&self.value.focus_handle(cx), cx);
        }
        cx.notify();
    }
}

fn input_box(input: &Entity<TextInput>, theme: &Theme) -> impl IntoElement {
    div()
        .h(px(26.))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.input_bg)
        .text_color(theme.text)
        .child(input.clone())
}

fn checkbox(checked: bool, theme: &Theme) -> impl IntoElement {
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(if checked {
            theme.accent
        } else {
            theme.text_faint
        })
        .when(checked, |el| {
            el.bg(theme.accent)
                .text_color(theme.accent_text)
                .text_size(px(10.))
                .child("\u{2713}")
        })
}

fn section_label(text: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
        .child(text)
}

impl Render for FilterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let accent = theme.accent;
        let op_label = self.op.map(|op| op.label()).unwrap_or("None");
        let operands = self.op.map(|op| op.operands()).unwrap_or(0);
        let all_checked = self.all_visible_checked();
        let list_len = self.visible.len();
        let list_h = (list_len.max(1) as f32 * ROW).min(ROW * 8.);

        let op_menu = self.op_menu_open.then(|| {
            let mut items: Vec<gpui::AnyElement> = Vec::new();
            for (i, op) in std::iter::once(None)
                .chain(FilterOp::ALL.iter().copied().map(Some))
                .enumerate()
            {
                if op.is_some_and(|op| op.starts_group()) {
                    items.push(div().h(px(1.)).my_1().bg(theme.border).into_any_element());
                }
                let selected = op == self.op;
                items.push(
                    div()
                        .id(("op", i))
                        .h(px(22.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .when(selected, |el| el.text_color(accent))
                        .hover(move |s| s.bg(accent).text_color(gpui::white()))
                        .child(op.map(|op| op.label()).unwrap_or("None"))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.set_op(op, window, cx)),
                        )
                        .into_any_element(),
                );
            }
            div()
                .flex()
                .flex_col()
                .p_1()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg)
                .children(items)
        });

        div()
            .track_focus(&self.focus)
            .key_context("FilterPanel")
            .w(px(290.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .shadow_lg()
            .text_size(px(13.))
            .text_color(theme.text)
            .line_height(px(18.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(if self.column.is_empty() {
                                "Filter".to_string()
                            } else {
                                self.column.clone()
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        small_button("sort-asc", "\u{2191}  Sort A \u{2192} Z", &theme)
                            .flex_1()
                            .justify_center()
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(FilterPanelEvent::Sort(SortDir::Asc))
                            })),
                    )
                    .child(
                        small_button("sort-desc", "\u{2193}  Sort Z \u{2192} A", &theme)
                            .flex_1()
                            .justify_center()
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(FilterPanelEvent::Sort(SortDir::Desc))
                            })),
                    ),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .child(section_label("FILTER BY CONDITION", &theme))
            .child(
                div()
                    .id("op-select")
                    .h(px(26.))
                    .px_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if self.op_menu_open {
                        accent
                    } else {
                        theme.border
                    })
                    .bg(theme.input_bg)
                    .cursor_pointer()
                    .child(div().flex_1().child(op_label))
                    .child(div().text_color(theme.text_muted).child("\u{25be}"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.op_menu_open = !this.op_menu_open;
                        cx.notify();
                    })),
            )
            .children(op_menu)
            .when(operands >= 1 && !self.op_menu_open, |el| {
                el.child(input_box(&self.value, &theme))
            })
            .when(operands >= 2 && !self.op_menu_open, |el| {
                el.child(input_box(&self.value2, &theme))
            })
            .child(div().h(px(1.)).bg(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .child(section_label("FILTER BY VALUES", &theme))
                    .child(div().text_size(px(11.)).text_color(theme.text_faint).child(
                        if self.distinct > self.values.len() {
                            format!(
                                "first {} of {}",
                                fmt_int(self.values.len()),
                                fmt_int(self.distinct)
                            )
                        } else {
                            format!("{} distinct", fmt_int(self.distinct))
                        },
                    )),
            )
            .child(input_box(&self.search, &theme))
            .child(
                div()
                    .id("select-all")
                    .h(px(ROW))
                    .px_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .child(checkbox(all_checked, &theme))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Select All"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_all(cx))),
            )
            .child(
                div()
                    .h(px(list_h))
                    .border_1()
                    .border_color(theme.grid_line)
                    .rounded(px(6.))
                    .child(if list_len == 0 {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.text_faint)
                            .child("No matching values")
                            .into_any_element()
                    } else {
                        uniform_list(
                            "filter-values",
                            list_len,
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                let theme = Theme::get(cx);
                                range
                                    .map(|i| {
                                        let ix = this.visible[i];
                                        let (value, count) = &this.values[ix];
                                        let checked = this.checked[ix];
                                        let label = if value.trim().is_empty() {
                                            "(Blanks)".to_string()
                                        } else {
                                            value.replace(['\n', '\r'], " ")
                                        };
                                        div()
                                            .id(("value", ix))
                                            .h(px(ROW))
                                            .px_2()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.hover))
                                            .child(checkbox(checked, &theme))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .when(value.trim().is_empty(), |el| {
                                                        el.italic().text_color(theme.text_muted)
                                                    })
                                                    .child(label),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(theme.text_faint)
                                                    .child(fmt_int(*count)),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.checked[ix] = !this.checked[ix];
                                                cx.notify();
                                            }))
                                    })
                                    .collect()
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.list_scroll)
                        .into_any_element()
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .child(
                        div()
                            .id("clear-filter")
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .child("Clear")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(FilterPanelEvent::Apply(Filter::default()))
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        small_button("cancel-filter", "Cancel", &theme)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FilterPanelEvent::Close))),
                    )
                    .child(
                        primary_button("apply-filter", "Apply", &theme)
                            .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                    ),
            )
    }
}
