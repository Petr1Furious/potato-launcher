use std::{
    cell::{Cell, RefCell},
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, SystemTime},
};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, IndexPath, Selectable as _, Sizable, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    h_flex,
    input::{
        Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
        InputHighlighterFactory, Rope, RopeExt as _, Search,
    },
    searchable_list::{SearchableGroup, SearchableVec},
    select::{Select, SelectEvent, SelectItem, SelectState},
    v_flex,
};
use gpui_kit::{
    AppContext, ClipboardItem, Context, Entity, HighlightStyle, Hsla, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Task, Window, div, prelude::*, relative, rems,
};
use launcher_bridge::{BackendSender, MessageToBackend, NotificationLevel};
use launcher_i18n as t;

use crate::entity::notification::NotificationEntries;

const MAX_LINES: usize = 50_000;
const LOG_LANGUAGE: &str = "potato-log";
const LIST_REFRESH_TICKS: u32 = 10;
const SCROLL_TO_END: f64 = -1e9;

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Plain,
    Debug,
    Warn,
    Error,
}

#[derive(Clone, Copy)]
struct LineKind {
    level: Level,
    trace: bool,
}

fn tagged_level(line: &str) -> Option<Level> {
    [
        ("/FATAL]", Level::Error),
        ("/ERROR]", Level::Error),
        ("/WARN]", Level::Warn),
        ("/DEBUG]", Level::Debug),
        ("/TRACE]", Level::Debug),
        ("/INFO]", Level::Plain),
    ]
    .into_iter()
    .find(|(tag, _)| line.contains(tag))
    .map(|(_, level)| level)
}

fn is_exception_header(line: &str) -> bool {
    let name = line.split([':', ' ']).next().unwrap_or_default();
    name.contains('.')
        && (name.ends_with("Exception") || name.ends_with("Error") || name.ends_with("Throwable"))
}

fn classify(line: &str, prev: Option<LineKind>) -> LineKind {
    let trimmed = line.trim_start();
    let indented = trimmed.len() != line.len();
    let trace = (indented && (trimmed.starts_with("at ") || trimmed.starts_with("... ")))
        || trimmed.starts_with("Caused by:")
        || trimmed.starts_with("Suppressed:");
    let inherited = prev.map_or(Level::Plain, |prev| prev.level);
    let level = if let Some(level) = tagged_level(line) {
        level
    } else if trace || (indented && !trimmed.is_empty()) {
        inherited
    } else if is_exception_header(line) || line.starts_with("Exception in thread") {
        Level::Error
    } else if line.starts_with("WARNING") {
        Level::Warn
    } else {
        Level::Plain
    };
    LineKind { level, trace }
}

fn classify_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
    mut prev: Option<LineKind>,
) -> Vec<LineKind> {
    lines
        .map(|line| {
            let kind = classify(line, prev);
            prev = Some(kind);
            kind
        })
        .collect()
}

fn line_starts(text: &str, base: usize, first_row: bool) -> Vec<usize> {
    first_row
        .then_some(base)
        .into_iter()
        .chain(text.match_indices('\n').map(|(index, _)| base + index + 1))
        .collect()
}

fn fold_ranges(kinds: &[LineKind]) -> Vec<FoldRange> {
    let mut folds = Vec::new();
    let mut row = 1;
    while row < kinds.len() {
        if kinds[row].trace && !kinds[row - 1].trace {
            let start = row - 1;
            let mut end = row;
            while end + 1 < kinds.len() && kinds[end + 1].trace {
                end += 1;
            }
            if end - start >= 2 {
                let next_starts_trace = kinds.get(end + 2).is_some_and(|kind| kind.trace);
                let last = if end + 1 < kinds.len() && !next_starts_trace {
                    end + 1
                } else {
                    end
                };
                folds.push(FoldRange::new(start, last));
            }
            row = end + 1;
        } else {
            row += 1;
        }
    }
    folds
}

#[derive(Clone, Copy)]
struct LevelColors {
    debug: Hsla,
    warn: Hsla,
    error: Hsla,
}

struct LogHighlighter {
    starts: Rc<RefCell<Vec<usize>>>,
    kinds: Rc<RefCell<Vec<LineKind>>>,
    folds: Rc<RefCell<Vec<FoldRange>>>,
    colored: Rc<Cell<bool>>,
    colors: LevelColors,
}

impl LogHighlighter {
    fn style(&self, row: usize) -> HighlightStyle {
        if !self.colored.get() {
            return HighlightStyle::default();
        }
        let color = match self.kinds.borrow().get(row).map(|kind| kind.level) {
            Some(Level::Debug) => self.colors.debug,
            Some(Level::Warn) => self.colors.warn,
            Some(Level::Error) => self.colors.error,
            _ => return HighlightStyle::default(),
        };
        HighlightStyle {
            color: Some(color),
            ..Default::default()
        }
    }
}

impl InputHighlighter for LogHighlighter {
    fn language(&self) -> SharedString {
        LOG_LANGUAGE.into()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        _text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let starts = self.starts.borrow();
        let mut styles: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut push = |segment: Range<usize>, style: HighlightStyle| match styles.last_mut() {
            Some((last, last_style)) if *last_style == style && last.end == segment.start => {
                last.end = segment.end;
            }
            _ => styles.push((segment, style)),
        };
        let mut pos = range.start;
        let mut row = starts
            .partition_point(|start| *start <= pos)
            .saturating_sub(1);
        while pos < range.end && row < starts.len() {
            let segment_end = starts
                .get(row + 1)
                .map_or(range.end, |next| (*next).min(range.end));
            if segment_end > pos {
                push(pos..segment_end, self.style(row));
                pos = segment_end;
            }
            row += 1;
        }
        if pos < range.end {
            push(pos..range.end, HighlightStyle::default());
        }
        styles
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        self.folds.borrow().clone()
    }

    fn fold_ranges_for_edit(&self, range: Range<usize>, text: &Rope) -> Vec<FoldRange> {
        let first = text.offset_to_point(range.start.min(text.len())).row;
        let last = text.offset_to_point(range.end.min(text.len())).row;
        self.folds
            .borrow()
            .iter()
            .filter(|fold| fold.start_line >= first && fold.start_line <= last)
            .copied()
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct LogFile {
    path: PathBuf,
    title: SharedString,
}

impl SelectItem for LogFile {
    type Value = PathBuf;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &PathBuf {
        &self.path
    }
}

type FileList = SearchableVec<SearchableGroup<LogFile>>;

fn file_entry(path: PathBuf) -> LogFile {
    let title = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    LogFile {
        path,
        title: title.into(),
    }
}

fn newest_first(paths: &mut [PathBuf]) {
    let modified = |path: &PathBuf| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    paths.sort_by_key(|path| std::cmp::Reverse(modified(path)));
}

fn files_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(&keep)
        })
        .collect();
    newest_first(&mut paths);
    paths
}

fn list_files(live_path: &Path, game_dir: &Path) -> Vec<SearchableGroup<LogFile>> {
    let mut groups = vec![
        SearchableGroup::new(t::logs::group_launcher()).item(LogFile {
            path: live_path.to_path_buf(),
            title: t::logs::current_run().into(),
        }),
    ];

    let mut game = files_in(&game_dir.join("logs"), |name| {
        name.ends_with(".log") || name.ends_with(".log.gz")
    });
    if let Some(latest) = game.iter().position(|path| path.ends_with("latest.log")) {
        let latest = game.remove(latest);
        game.insert(0, latest);
    }
    if !game.is_empty() {
        groups.push(
            SearchableGroup::new(t::logs::group_game()).items(game.into_iter().map(file_entry)),
        );
    }

    let mut crashes = files_in(&game_dir.join("crash-reports"), |name| {
        name.ends_with(".txt")
    });
    crashes.extend(files_in(game_dir, |name| {
        name.starts_with("hs_err_pid") && name.ends_with(".log")
    }));
    newest_first(&mut crashes);
    if !crashes.is_empty() {
        groups.push(
            SearchableGroup::new(t::logs::group_crashes())
                .items(crashes.into_iter().map(file_entry)),
        );
    }
    groups
}

fn file_paths(groups: &[SearchableGroup<LogFile>]) -> Vec<PathBuf> {
    groups
        .iter()
        .flat_map(|group| group.items.iter().map(|item| item.path.clone()))
        .collect()
}

fn is_gz(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "gz")
}

struct Loaded {
    text: String,
    partial: String,
    offset: u64,
    kinds: Vec<LineKind>,
    starts: Vec<usize>,
}

fn last_lines(text: &str, max: usize) -> &str {
    let mut count = 0;
    for (index, byte) in text.bytes().enumerate().rev().skip(1) {
        if byte == b'\n' {
            count += 1;
            if count == max {
                return &text[index + 1..];
            }
        }
    }
    text
}

fn read_log(path: &Path) -> Option<Loaded> {
    let mut bytes = Vec::new();
    let mut file = std::fs::File::open(path).ok()?;
    file.read_to_end(&mut bytes).ok()?;
    let offset = bytes.len() as u64;
    let mut text = if is_gz(path) {
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .read_to_string(&mut decoded)
            .ok()?;
        decoded
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    text = text.replace("\r\n", "\n");
    let partial = if is_gz(path) || text.ends_with('\n') {
        String::new()
    } else {
        match text.rfind('\n') {
            Some(index) => text.split_off(index + 1),
            None => std::mem::take(&mut text),
        }
    };
    let text = last_lines(&text, MAX_LINES);
    let text = text.strip_suffix('\n').unwrap_or(text).to_string();
    let (kinds, starts) = if text.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (
            classify_lines(text.split('\n'), None),
            line_starts(&text, 0, true),
        )
    };
    Some(Loaded {
        text,
        partial,
        offset,
        kinds,
        starts,
    })
}

pub struct LogView {
    live_path: PathBuf,
    game_dir: PathBuf,
    source: PathBuf,
    files: Entity<SelectState<FileList>>,
    file_paths: Vec<PathBuf>,
    editor: Entity<EditorState>,
    starts: Rc<RefCell<Vec<usize>>>,
    kinds: Rc<RefCell<Vec<LineKind>>>,
    folds: Rc<RefCell<Vec<FoldRange>>>,
    colored: Rc<Cell<bool>>,
    offset: u64,
    partial: String,
    missing: bool,
    loading: bool,
    keep_updating: bool,
    wrap: bool,
    collapse: bool,
    sender: BackendSender,
    notifications: Entity<NotificationEntries>,
    _poll: Task<()>,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LogView {
    pub fn new(
        live_path: PathBuf,
        game_dir: PathBuf,
        sender: BackendSender,
        notifications: Entity<NotificationEntries>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let starts = Rc::new(RefCell::new(Vec::new()));
        let kinds = Rc::new(RefCell::new(Vec::new()));
        let folds: Rc<RefCell<Vec<FoldRange>>> = Rc::new(RefCell::new(Vec::new()));
        let colored = Rc::new(Cell::new(true));
        let theme = cx.theme();
        let colors = LevelColors {
            debug: theme.muted_foreground,
            warn: theme.warning,
            error: theme.danger,
        };
        let pill_bg = theme.muted;
        let pill_fg = theme.muted_foreground;

        let factory: InputHighlighterFactory = {
            let starts = starts.clone();
            let kinds = kinds.clone();
            let folds = folds.clone();
            let colored = colored.clone();
            Rc::new(move |language| {
                (language == LOG_LANGUAGE).then(|| {
                    Box::new(LogHighlighter {
                        starts: starts.clone(),
                        kinds: kinds.clone(),
                        folds: folds.clone(),
                        colored: colored.clone(),
                        colors,
                    }) as Box<dyn InputHighlighter>
                })
            })
        };
        let placeholder_folds = folds.clone();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(LOG_LANGUAGE)
                .line_number(true)
                .soft_wrap(true)
                .searchable(true)
                .folding(true);
            state.set_scroll_beyond_last_line(Some(0), window, cx);
            state.set_highlighter_factory(factory, cx);
            state.set_readonly(true, cx);
            state.set_fold_placeholder_renderer(
                Some(Rc::new(move |line| {
                    let hidden = placeholder_folds
                        .borrow()
                        .iter()
                        .find(|fold| fold.start_line == line)
                        .map_or(0, |fold| fold.end_line - fold.start_line - 1);
                    div()
                        .ml_2()
                        .px_1p5()
                        .rounded_sm()
                        .bg(pill_bg)
                        .text_color(pill_fg)
                        .child(t::logs::more_lines(hidden))
                        .into_any_element()
                })),
                cx,
            );
            state
        });

        let groups = list_files(&live_path, &game_dir);
        let file_paths = file_paths(&groups);
        let files = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(groups),
                Some(IndexPath::default()),
                window,
                cx,
            )
            .searchable(true)
        });
        let subscriptions = vec![cx.subscribe_in(
            &files,
            window,
            |this, _, event: &SelectEvent<FileList>, window, cx| {
                let SelectEvent::Confirm(Some(path)) = event else {
                    return;
                };
                if *path != this.source {
                    this.source = path.clone();
                    this.load(window, cx);
                }
            },
        )];

        let poll = cx.spawn_in(window, async move |this, cx| {
            let mut tick = 0u32;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                tick = tick.wrapping_add(1);
                let refresh = tick.is_multiple_of(LIST_REFRESH_TICKS);
                if this
                    .update_in(cx, |this, window, cx| {
                        if refresh {
                            this.refresh_files(window, cx);
                        }
                        this.poll(window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let mut view = Self {
            source: live_path.clone(),
            live_path,
            game_dir,
            files,
            file_paths,
            editor,
            starts,
            kinds,
            folds,
            colored,
            offset: 0,
            partial: String::new(),
            missing: false,
            loading: false,
            keep_updating: true,
            wrap: true,
            collapse: true,
            sender,
            notifications,
            _poll: poll,
            _load: None,
            _subscriptions: subscriptions,
        };
        view.load(window, cx);
        view
    }

    fn refresh_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let groups = list_files(&self.live_path, &self.game_dir);
        let paths = file_paths(&groups);
        if paths == self.file_paths {
            return;
        }
        self.file_paths = paths;
        let source = self.source.clone();
        self.files.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(groups), window, cx);
            state.set_selected_value(&source, window, cx);
        });
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.source.clone();
        self.loading = true;
        let task = cx.background_executor().spawn(async move {
            let loaded = read_log(&path);
            (path, loaded)
        });
        self._load = Some(cx.spawn_in(window, async move |this, cx| {
            let (path, loaded) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if path != this.source {
                    return;
                }
                this.loading = false;
                this.missing = loaded.is_none();
                let loaded = loaded.unwrap_or(Loaded {
                    text: String::new(),
                    partial: String::new(),
                    offset: 0,
                    kinds: Vec::new(),
                    starts: Vec::new(),
                });
                this.offset = loaded.offset;
                this.partial = loaded.partial;
                *this.kinds.borrow_mut() = loaded.kinds;
                *this.starts.borrow_mut() = loaded.starts;
                this.update_folds();
                let folds = this.folds.borrow().clone();
                let collapse = this.collapse;
                this.editor.update(cx, |state, cx| {
                    state.set_value(loaded.text, window, cx);
                    state.set_highlighter(LOG_LANGUAGE, cx);
                    state.apply_highlighter_fold_candidates(folds.clone(), cx);
                    if collapse {
                        for fold in &folds {
                            state.set_folded(fold.start_line, true, cx);
                        }
                    }
                    state.set_scroll_y_f64(SCROLL_TO_END, cx);
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn update_folds(&self) {
        *self.folds.borrow_mut() = fold_ranges(&self.kinds.borrow());
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || !self.keep_updating || is_gz(&self.source) {
            return;
        }
        let Ok(mut file) = std::fs::File::open(&self.source) else {
            return;
        };
        let Ok(len) = file.metadata().map(|meta| meta.len()) else {
            return;
        };
        if len < self.offset || (self.missing && len > 0) {
            self.load(window, cx);
            return;
        }
        if len == self.offset {
            return;
        }
        let mut chunk = Vec::new();
        if file.seek(SeekFrom::Start(self.offset)).is_err() || file.read_to_end(&mut chunk).is_err()
        {
            return;
        }
        self.offset += chunk.len() as u64;
        self.partial
            .push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
        let Some(newline) = self.partial.rfind('\n') else {
            return;
        };
        let rest = self.partial.split_off(newline + 1);
        let added = std::mem::replace(&mut self.partial, rest);
        self.append(added, window, cx);
    }

    fn append(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let lines = text.strip_suffix('\n').unwrap_or(&text).to_string();
        let continues = !self.kinds.borrow().is_empty();
        let total = self.kinds.borrow().len() + lines.split('\n').count();
        if total > MAX_LINES + MAX_LINES / 5 {
            let mut all = self.editor.read(cx).text().to_string();
            all.push('\n');
            all.push_str(&lines);
            let kept = last_lines(&all, MAX_LINES).to_string();
            *self.kinds.borrow_mut() = classify_lines(kept.split('\n'), None);
            *self.starts.borrow_mut() = line_starts(&kept, 0, true);
            self.update_folds();
            let folds = self.folds.borrow().clone();
            let collapse = self.collapse;
            self.editor.update(cx, |state, cx| {
                let at_bottom = Self::at_bottom(state);
                let scroll = state.scroll_y_f64();
                state.set_value(kept, window, cx);
                state.apply_highlighter_fold_candidates(folds.clone(), cx);
                if collapse {
                    for fold in &folds {
                        state.set_folded(fold.start_line, true, cx);
                    }
                }
                state.set_scroll_y_f64(if at_bottom { SCROLL_TO_END } else { scroll }, cx);
            });
            return;
        }

        let previous_folds: Vec<usize> = self
            .folds
            .borrow()
            .iter()
            .map(|fold| fold.start_line)
            .collect();
        let folded_before: Vec<usize> = {
            let display_map = self.editor.read(cx).display_map();
            previous_folds
                .iter()
                .copied()
                .filter(|start| display_map.is_folded_at(*start))
                .collect()
        };
        let prev = self.kinds.borrow().last().copied();
        let added = classify_lines(lines.split('\n'), prev);
        let text = if continues {
            format!("\n{lines}")
        } else {
            lines
        };
        self.kinds.borrow_mut().extend(added);
        let base = self.editor.read(cx).text().len();
        self.starts
            .borrow_mut()
            .extend(line_starts(&text, base, !continues));
        self.update_folds();
        let folds = self.folds.borrow().clone();
        let collapse = self.collapse;
        self.editor.update(cx, |state, cx| {
            let at_bottom = Self::at_bottom(state);
            let scroll = state.scroll_y_f64();
            let selection = state.selected_range();
            let end = state.text().len();
            state.set_selected_range(end..end, cx);
            state.insert(text, window, cx);
            state.set_selected_range(selection, cx);
            state.apply_highlighter_fold_candidates(folds.clone(), cx);
            for fold in &folds {
                let start = fold.start_line;
                let is_new = !previous_folds.contains(&start);
                if folded_before.contains(&start) || (collapse && is_new) {
                    state.set_folded(start, false, cx);
                    state.set_folded(start, true, cx);
                }
            }
            state.set_scroll_y_f64(if at_bottom { SCROLL_TO_END } else { scroll }, cx);
        });
        cx.notify();
    }

    fn at_bottom(state: &EditorState) -> bool {
        let viewport = f64::from(f32::from(state.input_bounds().size.height));
        let line = state
            .line_height()
            .map_or(20.0, |height| f64::from(f32::from(height)));
        let max_scroll = (state.content_height_f64() - viewport).max(0.0);
        viewport == 0.0 || -state.scroll_y_f64() >= max_scroll - line * 2.0
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.kinds.borrow_mut().clear();
        self.starts.borrow_mut().clear();
        self.folds.borrow_mut().clear();
        self.editor.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.apply_highlighter_fold_candidates(Vec::new(), cx);
        });
        cx.notify();
    }

    fn set_collapse(&mut self, collapse: bool, cx: &mut Context<Self>) {
        self.collapse = collapse;
        let folds = self.folds.borrow().clone();
        self.editor.update(cx, |state, cx| {
            if collapse {
                for fold in &folds {
                    state.set_folded(fold.start_line, true, cx);
                }
            } else {
                state.unfold_all(cx);
            }
        });
        cx.notify();
    }

    fn confirm_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content: std::sync::Arc<str> = self.editor.read(cx).text().to_string().into();
        let sender = self.sender.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let sender = sender.clone();
            let content = content.clone();
            alert
                .title(t::logs::upload_title())
                .description(t::logs::upload_description())
                .ok_text(t::logs::upload_confirm())
                .ok_variant(ButtonVariant::Primary)
                .show_cancel(true)
                .cancel_text(t::common::cancel())
                .on_ok(move |_, _, _| {
                    sender.send(MessageToBackend::UploadLog(content.clone()));
                    true
                })
        });
    }

    fn tool(id: &'static str, icon: IconName, tooltip: impl Into<SharedString>) -> Button {
        Button::new(id).small().ghost().icon(icon).tooltip(tooltip)
    }
}

impl Render for LogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.editor.read(cx).text().len() == 0;
        let live_capable = !is_gz(&self.source);
        let separator = || div().w(gpui_kit::px(1.)).h_4().mx_1().bg(cx.theme().border);

        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div().w(rems(16.)).child(
                            Select::new(&self.files)
                                .small()
                                .menu_width(rems(22.))
                                .menu_max_h(rems(24.)),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        Self::tool("log-find", IconName::Search, t::logs::find())
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor.update(cx, |state, cx| state.focus(window, cx));
                                window.defer(cx, |window, cx| {
                                    window.dispatch_action(Box::new(Search), cx)
                                });
                            })),
                    )
                    .child(
                        Self::tool("log-wrap", IconName::TextWrap, t::logs::wrap())
                            .selected(self.wrap)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.wrap = !this.wrap;
                                let wrap = this.wrap;
                                this.editor
                                    .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::tool("log-color", IconName::Palette, t::logs::color())
                            .selected(self.colored.get())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.colored.set(!this.colored.get());
                                this.editor.update(cx, |state, cx| {
                                    state.set_highlighter(LOG_LANGUAGE, cx)
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::tool("log-collapse", IconName::ListCollapse, t::logs::collapse())
                            .selected(self.collapse)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_collapse(!this.collapse, cx);
                            })),
                    )
                    .child(separator())
                    .child(
                        Self::tool("log-follow", IconName::Radio, t::logs::keep_updating())
                            .selected(self.keep_updating && live_capable)
                            .disabled(!live_capable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.keep_updating = !this.keep_updating;
                                if this.keep_updating {
                                    this.poll(window, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::tool("log-bottom", IconName::ArrowDownToLine, t::logs::bottom())
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editor.update(cx, |state, cx| {
                                    state.set_scroll_y_f64(SCROLL_TO_END, cx)
                                });
                            })),
                    )
                    .child(
                        Self::tool("log-clear", IconName::Eraser, t::logs::clear())
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
                    )
                    .child(separator())
                    .child(
                        Self::tool("log-copy", IconName::Copy, t::logs::copy())
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.editor.read(cx).text().to_string();
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                this.notifications.update(cx, |entries, cx| {
                                    entries.push(
                                        NotificationLevel::Info,
                                        t::notifications::copied(),
                                        cx,
                                    )
                                });
                            })),
                    )
                    .child(
                        Self::tool("log-upload", IconName::Upload, t::logs::upload())
                            .disabled(empty)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_upload(window, cx)),
                            ),
                    )
                    .child(
                        Self::tool("log-open", IconName::ExternalLink, t::logs::open_file())
                            .disabled(self.missing)
                            .on_click(cx.listener(|this, _, _, _| {
                                let _ = crate::pages::common::open_path(&this.source);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(Editor::new(&self.editor).h(relative(1.)))
                    .when(empty && !self.loading, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.missing && self.source == self.live_path {
                                    t::logs::empty()
                                } else {
                                    t::logs::empty_file()
                                }),
                        )
                    }),
            )
    }
}
