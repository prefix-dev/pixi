//! Rendering of the terminal output of `pixi global`.
//!
//! Every subcommand describes what it did as a list of [`EnvReport`]s, one per
//! environment, and hands them to [`print()`]. Rendering is a pure function of
//! the report and the [`RenderOptions`], so it can be tested without a
//! terminal.
//!
//! A block looks like this:
//!
//! ```text
//! (installed) ripgrep 15.2.0
//! ├── exposed       + rg
//! └── completions   + rg
//! ```
//!
//! Rows are children of their own header, so a block is self-contained and can
//! be printed as soon as its environment is done. The labels are the keys of
//! `pixi-global.toml`, except for `completions` and `size`, which describe the
//! result rather than the manifest.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use console::Style;
use pixi_consts::consts;

/// Width of the label column, sized to the longest label.
const LABEL_WIDTH: usize = "dependencies".len();
/// Indentation of the body of a failure, which is prose rather than rows.
const INDENT: usize = 2;
/// Width of the status column, sized to the longest status. Names are padded
/// out past it so that headers line up down a run without knowing how wide the
/// widest one will be.
const STATUS_WIDTH: usize = "(installed)".len();
/// Drawn in front of a row, and in front of the lines that continue it.
const BRANCH: &str = "├── ";
const LAST_BRANCH: &str = "└── ";
const TRUNK: &str = "│   ";
const NO_TRUNK: &str = "    ";
/// Gap between the label column and the value column.
const GAP: usize = 2;
/// Item lists longer than this get one item per line instead of being joined
/// into a wrapped line.
const ITEMS_PER_LINE: usize = 4;

/// How much of the report to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Print nothing but failures.
    Quiet,
    /// Print the whole report.
    Normal,
}

static VERBOSITY: AtomicU8 = AtomicU8::new(Verbosity::Normal as u8);

/// Whether anything has been printed yet, so blocks after the first can be
/// separated by a blank line without the callers having to keep track.
static PRINTED: AtomicBool = AtomicBool::new(false);

/// Set the verbosity of the reports of this process.
pub fn set_verbosity(verbosity: Verbosity) {
    VERBOSITY.store(verbosity as u8, Ordering::Relaxed);
}

/// The verbosity of the reports of this process.
pub fn verbosity() -> Verbosity {
    match VERBOSITY.load(Ordering::Relaxed) {
        value if value == Verbosity::Quiet as u8 => Verbosity::Quiet,
        _ => Verbosity::Normal,
    }
}

/// What happened to an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvStatus {
    /// The environment didn't exist before.
    Installed,
    /// An existing environment changed in any way.
    Updated,
    /// The environment was removed.
    Removed,
    /// The environment was considered but nothing changed.
    Unchanged,
    /// The environment couldn't be processed.
    Failed,
}

impl EnvStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            EnvStatus::Installed => "installed",
            EnvStatus::Updated => "updated",
            EnvStatus::Removed => "removed",
            EnvStatus::Unchanged => "unchanged",
            EnvStatus::Failed => "failed",
        }
    }

    /// The color of the status word, matching the [`Marker`] of the change it
    /// describes: green adds, yellow changes, red takes away.
    fn style(self) -> Style {
        match self {
            EnvStatus::Installed => Style::new().green(),
            EnvStatus::Updated => Style::new().yellow(),
            EnvStatus::Removed => Style::new().red(),
            EnvStatus::Failed => Style::new().red().bold(),
            EnvStatus::Unchanged => Style::new().dim(),
        }
    }
}

/// What happened to a single item of a row.
///
/// The markers are ASCII: they are the only signal left when colors are
/// disabled, so they can't depend on the terminal rendering a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Added,
    Removed,
    Changed,
    /// The item describes state rather than a change, as in `pixi global list`.
    None,
}

impl Marker {
    fn as_str(self) -> &'static str {
        match self {
            Marker::Added => "+",
            Marker::Removed => "-",
            Marker::Changed => "~",
            Marker::None => "",
        }
    }

    fn style(self) -> Style {
        match self {
            Marker::Added => Style::new().green(),
            Marker::Removed => Style::new().red(),
            Marker::Changed => Style::new().yellow(),
            Marker::None => Style::new(),
        }
    }
}

/// The label of a row, matching the key of `pixi-global.toml` where there is
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Dependencies,
    Exposed,
    Shortcuts,
    Completions,
    Channels,
    Platform,
    Size,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Dependencies => "dependencies",
            Label::Exposed => "exposed",
            Label::Shortcuts => "shortcuts",
            Label::Completions => "completions",
            Label::Channels => "channels",
            Label::Platform => "platform",
            Label::Size => "size",
        }
    }
}

/// What an item refers to, which decides how it is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Package,
    Exposed,
    Plain,
    /// An aggregate such as `12 added, 3 changed, 1 removed`.
    Summary,
}

/// A single entry of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub marker: Marker,
    pub kind: ItemKind,
    pub name: String,
    /// Trails the name, dimmed: a version, a version transition or the target
    /// of an exposed mapping.
    pub detail: Option<String>,
}

impl Item {
    pub fn package(marker: Marker, name: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            marker,
            kind: ItemKind::Package,
            name: name.into(),
            detail,
        }
    }

    pub fn exposed(marker: Marker, name: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            marker,
            kind: ItemKind::Exposed,
            name: name.into(),
            detail,
        }
    }

    pub fn plain(marker: Marker, name: impl Into<String>) -> Self {
        Self {
            marker,
            kind: ItemKind::Plain,
            name: name.into(),
            detail: None,
        }
    }

    pub fn summary(name: impl Into<String>) -> Self {
        Self {
            marker: Marker::None,
            kind: ItemKind::Summary,
            name: name.into(),
            detail: None,
        }
    }
}

/// A labelled line of a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: Label,
    pub items: Vec<Item>,
}

impl Row {
    pub fn new(label: Label, items: Vec<Item>) -> Self {
        Self { label, items }
    }
}

/// Everything that is reported about one environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvReport {
    pub name: String,
    /// Only set for environments that hold a single dependency named after the
    /// environment, where a `dependencies` row would just repeat the name.
    pub version: Option<String>,
    /// Absent in `pixi global list`, which describes state rather than change.
    pub status: Option<EnvStatus>,
    pub rows: Vec<Row>,
    /// Why an environment failed, rendered by miette. Only set on the entries
    /// of the closing section, never on the marker printed in place.
    pub diagnostic: Option<String>,
}

impl EnvReport {
    pub fn new(
        name: impl Into<String>,
        version: Option<String>,
        status: Option<EnvStatus>,
    ) -> Self {
        Self {
            name: name.into(),
            version,
            status,
            rows: Vec::new(),
            diagnostic: None,
        }
    }

    pub fn with_rows(mut self, rows: Vec<Row>) -> Self {
        self.rows = rows;
        self
    }

    /// A report for an environment that couldn't be processed. The reason is
    /// reported at the end of the run, not here.
    pub fn failed(name: impl Into<String>) -> Self {
        Self::new(name, None, Some(EnvStatus::Failed))
    }

    /// An entry of the closing section: the environment name and why it
    /// failed. No status word, since the marker above already said `failed`.
    pub fn reason(name: impl Into<String>, diagnostic: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            status: None,
            rows: Vec::new(),
            diagnostic: Some(diagnostic.into()),
        }
    }

    /// Whether this report is about a failed environment, either as the marker
    /// printed in place or as its reason in the closing section.
    fn is_failure(&self) -> bool {
        self.status == Some(EnvStatus::Failed) || self.diagnostic.is_some()
    }
}

/// How to turn a report into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderOptions {
    /// Where to wrap joined item lists. `None` doesn't wrap at all, which is
    /// what piped output wants.
    pub width: Option<usize>,
    pub color: bool,
}

impl RenderOptions {
    /// The options matching stderr, where the reports go.
    pub fn from_terminal() -> Self {
        let term = console::Term::stderr();
        Self {
            width: term.is_term().then(|| term.size().1 as usize),
            color: console::colors_enabled_stderr(),
        }
    }

    /// The options matching the terminal the queryable output goes to.
    pub fn for_stdout() -> Self {
        let term = console::Term::stdout();
        Self {
            width: term.is_term().then(|| term.size().1 as usize),
            color: console::colors_enabled(),
        }
    }
}

fn styled(style: &Style, color: bool, text: &str) -> String {
    style
        .clone()
        .force_styling(color)
        .apply_to(text)
        .to_string()
}

/// Render a single block, without a trailing newline.
pub fn render(report: &EnvReport, options: &RenderOptions) -> String {
    let mut lines = vec![render_header(report, options)];

    let last = report.rows.len().saturating_sub(1);
    for (index, row) in report.rows.iter().enumerate() {
        lines.extend(render_row(row, index == last, options));
    }

    if let Some(diagnostic) = &report.diagnostic {
        lines.extend(indent_diagnostic(diagnostic));
    }

    lines.join("\n")
}

/// Render several blocks, separated by a blank line.
pub fn render_all(reports: &[EnvReport], options: &RenderOptions) -> String {
    reports
        .iter()
        .map(|report| render(report, options))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Render the line counting the environments left out of the report because
/// nothing changed. `secondary` dims it, which suits a line trailing the blocks
/// it belongs to but not one that is the whole answer.
pub fn render_unchanged_summary(count: usize, options: &RenderOptions, secondary: bool) -> String {
    let text = if count == 1 {
        "1 environment unchanged".to_string()
    } else {
        format!("{count} environments unchanged")
    };
    let style = if secondary {
        Style::new().dim()
    } else {
        Style::new()
    };
    styled(&style, options.color, &text)
}

/// Render the heading that opens the closing section, marking where the reasons
/// of the failed environments start. Over a long run they sit a screen of
/// scrollback away from the markers they belong to.
pub fn render_failure_heading(action: &str, count: usize, options: &RenderOptions) -> String {
    let text = if count == 1 {
        format!("1 environment failed to {action}:")
    } else {
        format!("{count} environments failed to {action}:")
    };
    styled(&EnvStatus::Failed.style(), options.color, &text)
}

/// Split off the escape sequences a line starts with.
///
/// Miette puts the color before the indentation, so the spaces can only be
/// found after stepping over it.
fn split_leading_ansi(line: &str) -> (&str, &str) {
    let mut index = 0;
    while line[index..].starts_with('\u{1b}') {
        let Some(end) = line[index..].find(|character: char| character.is_ascii_alphabetic())
        else {
            break;
        };
        index += end + 1;
    }
    line.split_at(index)
}

/// Whether a line carries nothing but color and whitespace.
///
/// Miette paints the indentation of its wrapped lines, so a "blank" line can
/// arrive as an escape sequence, four spaces and a reset.
fn is_blank(line: &str) -> bool {
    console::strip_ansi_codes(line).trim().is_empty()
}

/// Re-indent a rendered diagnostic to sit at the same column as the rows.
///
/// Miette indents its own output, so the indentation it brought along is
/// stripped rather than added to, and blank lines around it are dropped.
fn indent_diagnostic(diagnostic: &str) -> Vec<String> {
    let lines: Vec<&str> = diagnostic
        .lines()
        .skip_while(|line| is_blank(line))
        .collect();
    let lines = match lines.iter().rposition(|line| !is_blank(line)) {
        Some(last) => &lines[..=last],
        None => return Vec::new(),
    };

    // Only ASCII spaces count as indentation: measuring bytes of arbitrary
    // whitespace would slice a multi-byte character in half.
    let leading_spaces = |line: &str| {
        let (_, rest) = split_leading_ansi(line);
        rest.len() - rest.trim_start_matches(' ').len()
    };

    let existing = lines
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| leading_spaces(line))
        .min()
        .unwrap_or(0);

    let indent = " ".repeat(INDENT);
    lines
        .iter()
        .map(|line| {
            if is_blank(line) {
                return String::new();
            }
            let (prefix, rest) = split_leading_ansi(line);
            let strip = existing.min(leading_spaces(line));
            format!("{indent}{prefix}{}", &rest[strip..])
                .trim_end()
                .to_string()
        })
        .collect()
}

fn render_header(report: &EnvReport, options: &RenderOptions) -> String {
    let mut header = String::new();

    if let Some(status) = report.status {
        // Padded before styling: escape sequences carry no width, so
        // formatting the styled string to a field would misalign it.
        let word = format!("({})", status.as_str());
        header.push_str(&styled(&status.style(), options.color, &word));
        header.push_str(&" ".repeat(STATUS_WIDTH.saturating_sub(word.len()) + 1));
    }

    header.push_str(&styled(
        &consts::ENVIRONMENT_STYLE,
        options.color,
        &report.name,
    ));

    if let Some(version) = &report.version {
        header.push(' ');
        header.push_str(&styled(&Style::new().dim(), options.color, version));
    }

    header
}

fn render_row(row: &Row, last: bool, options: &RenderOptions) -> Vec<String> {
    if row.items.is_empty() {
        return Vec::new();
    }

    let branch = if last { LAST_BRANCH } else { BRANCH };
    let label = format!(
        "{branch}{label:<LABEL_WIDTH$}{gap}",
        label = row.label.as_str(),
        gap = " ".repeat(GAP),
    );
    // Continuation lines keep the trunk running past them, unless this is the
    // last row and there is nothing left to connect to.
    let continuation = format!(
        "{}{}",
        if last { NO_TRUNK } else { TRUNK },
        " ".repeat(LABEL_WIDTH + GAP)
    );

    let rendered: Vec<String> = row
        .items
        .iter()
        .map(|item| render_item(item, options))
        .collect();

    // Past the threshold a joined line is more work to read than it saves, so
    // every item gets its own line.
    let values = if rendered.len() > ITEMS_PER_LINE {
        rendered
    } else {
        wrap(
            &rendered,
            options.width,
            console::measure_text_width(&continuation),
        )
    };

    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let prefix = if index == 0 { &label } else { &continuation };
            format!("{prefix}{value}")
        })
        .collect()
}

fn render_item(item: &Item, options: &RenderOptions) -> String {
    let mut rendered = String::new();

    if item.marker != Marker::None {
        rendered.push_str(&styled(
            &item.marker.style(),
            options.color,
            item.marker.as_str(),
        ));
        rendered.push(' ');
    }

    // A change report colors by what happened to the item, a state listing by
    // what the item is. Coloring a removed package green would collide with the
    // green that means "added" everywhere else.
    let name_style = match item.marker {
        Marker::None => match item.kind {
            ItemKind::Package => consts::CONDA_PACKAGE_STYLE.clone(),
            ItemKind::Exposed => consts::EXPOSED_NAME_STYLE.clone(),
            ItemKind::Plain => Style::new(),
            ItemKind::Summary => Style::new().dim(),
        },
        marker => marker.style(),
    };
    rendered.push_str(&styled(&name_style, options.color, &item.name));

    if let Some(detail) = &item.detail {
        // The target of an exposed mapping is a second name rather than an
        // annotation on the first, so it is not dimmed away.
        let detail_style = match (item.marker, item.kind) {
            (Marker::None, ItemKind::Exposed) => consts::EXPOSED_NAME_STYLE.clone(),
            (marker, ItemKind::Exposed) => marker.style(),
            _ => Style::new().dim(),
        };
        rendered.push(' ');
        rendered.push_str(&styled(&detail_style, options.color, detail));
    }

    rendered
}

/// Join items with `, `, breaking into further lines so that no line exceeds
/// `width` once `indent` columns are taken into account.
fn wrap(items: &[String], width: Option<usize>, indent: usize) -> Vec<String> {
    let joined = items.join(", ");
    let Some(width) = width else {
        return vec![joined];
    };
    if indent + console::measure_text_width(&joined) <= width {
        return vec![joined];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for (index, item) in items.iter().enumerate() {
        let piece = if index + 1 == items.len() {
            item.clone()
        } else {
            format!("{item},")
        };

        if current.is_empty() {
            current = piece;
        } else if indent
            + console::measure_text_width(&current)
            + 1
            + console::measure_text_width(&piece)
            <= width
        {
            current.push(' ');
            current.push_str(&piece);
        } else {
            lines.push(std::mem::take(&mut current));
            current = piece;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

/// Print a block to stderr, above any progress bars.
///
/// Failures are printed even when quiet, marker and reason alike: they are the
/// reason the command exits non-zero.
pub fn print(report: &EnvReport) {
    if verbosity() == Verbosity::Quiet && !report.is_failure() {
        return;
    }
    print_block(&render(report, &RenderOptions::from_terminal()));
}

/// Print the heading of the closing section. Never silenced: it introduces the
/// reasons, which are printed even when quiet.
pub fn print_failure_heading(action: &str, count: usize) {
    let options = RenderOptions::from_terminal();
    print_block(&render_failure_heading(action, count, &options));
}

/// Say that a command found nothing to do, for when there was not even an
/// unchanged environment to count. Stays quiet if anything has been printed.
pub fn print_nothing_to_do() {
    if verbosity() == Verbosity::Quiet || PRINTED.load(Ordering::Relaxed) {
        return;
    }
    let options = RenderOptions::from_terminal();
    print_block(&styled(&Style::new(), options.color, "Nothing to do."));
}

/// Print the line accounting for environments left out of the report.
pub fn print_unchanged_summary(count: usize) {
    if verbosity() == Verbosity::Quiet || count == 0 {
        return;
    }
    let options = RenderOptions::from_terminal();
    let secondary = PRINTED.load(Ordering::Relaxed);
    print_block(&render_unchanged_summary(count, &options, secondary));
}

fn print_block(block: &str) {
    // Blocks stream as environments finish, so the blank line that separates
    // them has to be emitted before the block rather than after it. Otherwise
    // the report always ends on an empty line.
    if PRINTED.swap(true, Ordering::Relaxed) {
        pixi_progress::println!("\n{block}");
    } else {
        pixi_progress::println!("{block}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> RenderOptions {
        RenderOptions {
            width: Some(80),
            color: false,
        }
    }

    fn installed() -> EnvReport {
        EnvReport::new(
            "ripgrep",
            Some("15.2.0".to_string()),
            Some(EnvStatus::Installed),
        )
        .with_rows(vec![Row::new(
            Label::Exposed,
            vec![Item::exposed(Marker::Added, "rg", None)],
        )])
    }

    #[test]
    fn renders_install() {
        insta::assert_snapshot!(render(&installed(), &options()), @r"
        (installed) ripgrep 15.2.0
        └── exposed       + rg
        ");
    }

    /// An environment named after its only dependency carries the whole change
    /// in its header, transition included, so no row repeats the name.
    #[test]
    fn renders_an_upgrade_of_the_environments_own_package() {
        let report = EnvReport::new(
            "ripgrep",
            Some("15.2.0 -> 15.3.0".to_string()),
            Some(EnvStatus::Updated),
        );

        insta::assert_snapshot!(render(&report, &options()), @"(updated)   ripgrep 15.2.0 -> 15.3.0");
    }

    /// An environment where only packages the manifest doesn't name moved still
    /// says that it changed, even though nothing is listed below it.
    #[test]
    fn renders_an_update_without_rows() {
        let report = EnvReport::new("dev", None, Some(EnvStatus::Updated));

        insta::assert_snapshot!(render(&report, &options()), @"(updated)   dev");
    }

    #[test]
    fn renders_update_with_dependency_changes() {
        let report = EnvReport::new("dev", None, Some(EnvStatus::Updated)).with_rows(vec![
            Row::new(
                Label::Dependencies,
                vec![
                    Item::package(
                        Marker::Changed,
                        "ripgrep",
                        Some("15.2.0 -> 15.3.0".to_string()),
                    ),
                    Item::package(Marker::Removed, "bat", Some("0.26.1".to_string())),
                ],
            ),
            Row::new(
                Label::Exposed,
                vec![Item::exposed(Marker::Removed, "bat", None)],
            ),
        ]);

        insta::assert_snapshot!(render(&report, &options()), @r"
        (updated)   dev
        ├── dependencies  ~ ripgrep 15.2.0 -> 15.3.0, - bat 0.26.1
        └── exposed       - bat
        ");
    }

    #[test]
    fn renders_failure_as_a_header_only() {
        insta::assert_snapshot!(
            render(&EnvReport::failed("nope"), &options()),
            @"(failed)    nope"
        );
    }

    #[test]
    fn renders_removal() {
        let report = EnvReport::new("ripgrep", None, Some(EnvStatus::Removed));
        insta::assert_snapshot!(render(&report, &options()), @"(removed)   ripgrep");
    }

    #[test]
    fn renders_unchanged() {
        let report = EnvReport::new(
            "ripgrep",
            Some("15.2.0".to_string()),
            Some(EnvStatus::Unchanged),
        );
        insta::assert_snapshot!(render(&report, &options()), @"(unchanged) ripgrep 15.2.0");
    }

    #[test]
    fn renders_list_without_markers() {
        let report = EnvReport::new("dev", None, None).with_rows(vec![
            Row::new(
                Label::Dependencies,
                vec![
                    Item::package(Marker::None, "bat", Some("0.26.1".to_string())),
                    Item::package(Marker::None, "ripgrep", Some("15.3.0".to_string())),
                ],
            ),
            Row::new(
                Label::Exposed,
                vec![
                    Item::exposed(Marker::None, "bat", None),
                    Item::exposed(Marker::None, "rg", None),
                ],
            ),
            Row::new(
                Label::Channels,
                vec![Item::plain(Marker::None, "conda-forge")],
            ),
        ]);

        insta::assert_snapshot!(render(&report, &options()), @r"
        dev
        ├── dependencies  bat 0.26.1, ripgrep 15.3.0
        ├── exposed       bat, rg
        └── channels      conda-forge
        ");
    }

    #[test]
    fn renders_exposed_mapping() {
        let report =
            EnvReport::new("ripgrep", None, Some(EnvStatus::Updated)).with_rows(vec![Row::new(
                Label::Exposed,
                vec![Item::exposed(
                    Marker::Added,
                    "rgx",
                    Some("-> rg".to_string()),
                )],
            )]);

        insta::assert_snapshot!(render(&report, &options()), @r"
        (updated)   ripgrep
        └── exposed       + rgx -> rg
        ");
    }

    #[test]
    fn puts_every_item_on_its_own_line_past_the_threshold() {
        let names = ["FFLMoni", "FdIOServer", "FdMoni", "FdSend", "fd"];
        let report = EnvReport::new("fd", Some("10.4.0".to_string()), Some(EnvStatus::Installed))
            .with_rows(vec![Row::new(
                Label::Exposed,
                names
                    .iter()
                    .map(|name| Item::exposed(Marker::Added, *name, None))
                    .collect(),
            )]);

        insta::assert_snapshot!(render(&report, &options()), @r"
        (installed) fd 10.4.0
        └── exposed       + FFLMoni
                          + FdIOServer
                          + FdMoni
                          + FdSend
                          + fd
        ");
    }

    #[test]
    fn wraps_a_short_list_that_does_not_fit() {
        let report = EnvReport::new("jupyterlab", None, None).with_rows(vec![Row::new(
            Label::Exposed,
            vec![
                Item::exposed(Marker::None, "jupyter-lab", None),
                Item::exposed(Marker::None, "jupyter-labextension", None),
                Item::exposed(Marker::None, "jupyter-labhub", None),
            ],
        )]);

        let narrow = RenderOptions {
            width: Some(60),
            color: false,
        };
        insta::assert_snapshot!(render(&report, &narrow), @r"
        jupyterlab
        └── exposed       jupyter-lab, jupyter-labextension,
                          jupyter-labhub
        ");
    }

    #[test]
    fn does_not_wrap_without_a_width() {
        let report = EnvReport::new("jupyterlab", None, None).with_rows(vec![Row::new(
            Label::Exposed,
            vec![
                Item::exposed(Marker::None, "jupyter-lab", None),
                Item::exposed(Marker::None, "jupyter-labextension", None),
                Item::exposed(Marker::None, "jupyter-labhub", None),
            ],
        )]);

        insta::assert_snapshot!(
            render(&report, &RenderOptions::default()),
            @r"
        jupyterlab
        └── exposed       jupyter-lab, jupyter-labextension, jupyter-labhub
        "
        );
    }

    #[test]
    fn markers_survive_without_color() {
        let report =
            EnvReport::new("dev", None, Some(EnvStatus::Updated)).with_rows(vec![Row::new(
                Label::Dependencies,
                vec![
                    Item::package(Marker::Added, "bat", Some("0.27.0".to_string())),
                    Item::package(Marker::Removed, "fd", Some("10.4.0".to_string())),
                    Item::package(Marker::Changed, "rg", Some("15.2.0 -> 15.3.0".to_string())),
                ],
            )]);

        let rendered = render(&report, &options());
        assert!(!rendered.contains('\u{1b}'), "{rendered}");
        insta::assert_snapshot!(rendered, @r"
        (updated)   dev
        └── dependencies  + bat 0.27.0, - fd 10.4.0, ~ rg 15.2.0 -> 15.3.0
        ");
    }

    #[test]
    fn colors_the_marker_and_the_status() {
        let report = EnvReport::new(
            "ripgrep",
            Some("15.2.0".to_string()),
            Some(EnvStatus::Installed),
        )
        .with_rows(vec![Row::new(
            Label::Exposed,
            vec![Item::exposed(Marker::Added, "rg", None)],
        )]);

        let rendered = render(
            &report,
            &RenderOptions {
                width: Some(80),
                color: true,
            },
        );

        // Magenta environment, dim version, green status, and a green marker
        // carrying its name along with it.
        insta::assert_snapshot!(rendered.replace('\u{1b}', "\\e"), @r"
        \e[32m(installed)\e[0m \e[35mripgrep\e[0m \e[2m15.2.0\e[0m
        └── exposed       \e[32m+\e[0m \e[32mrg\e[0m
        ");
    }

    /// A removal is drawn in red throughout, since green means an addition.
    #[test]
    fn colors_a_removal_without_using_the_color_of_an_addition() {
        let report = EnvReport::new("dev", None, Some(EnvStatus::Removed)).with_rows(vec![
            Row::new(
                Label::Dependencies,
                vec![Item::package(
                    Marker::Removed,
                    "bat",
                    Some("0.26.1".to_string()),
                )],
            ),
            Row::new(
                Label::Exposed,
                vec![Item::exposed(Marker::Removed, "bat", None)],
            ),
        ]);

        let rendered = render(
            &report,
            &RenderOptions {
                width: Some(80),
                color: true,
            },
        );

        insta::assert_snapshot!(rendered.replace('\u{1b}', "\\e"), @r"
        \e[31m(removed)\e[0m   \e[35mdev\e[0m
        ├── dependencies  \e[31m-\e[0m \e[31mbat\e[0m \e[2m0.26.1\e[0m
        └── exposed       \e[31m-\e[0m \e[31mbat\e[0m
        ");
    }

    /// Without a marker there is no change to color by, so the item falls back
    /// to the color of what it is. That is what `pixi global list` renders.
    #[test]
    fn colors_a_state_listing_by_what_the_items_are() {
        let report = EnvReport::new("dev", None, None).with_rows(vec![
            Row::new(
                Label::Dependencies,
                vec![Item::package(
                    Marker::None,
                    "bat",
                    Some("0.26.1".to_string()),
                )],
            ),
            Row::new(
                Label::Exposed,
                vec![Item::exposed(Marker::None, "bat", None)],
            ),
        ]);

        let rendered = render(
            &report,
            &RenderOptions {
                width: Some(80),
                color: true,
            },
        );

        insta::assert_snapshot!(rendered.replace('\u{1b}', "\\e"), @r"
        \e[35mdev\e[0m
        ├── dependencies  \e[32mbat\e[0m \e[2m0.26.1\e[0m
        └── exposed       \e[33mbat\e[0m
        ");
    }

    #[test]
    fn renders_several_blocks_with_a_blank_line_between_them() {
        let second = EnvReport::new(
            "bat",
            Some("0.27.0".to_string()),
            Some(EnvStatus::Installed),
        )
        .with_rows(vec![Row::new(
            Label::Exposed,
            vec![Item::exposed(Marker::Added, "bat", None)],
        )]);

        insta::assert_snapshot!(render_all(&[installed(), second], &options()), @r"
        (installed) ripgrep 15.2.0
        └── exposed       + rg

        (installed) bat 0.27.0
        └── exposed       + bat
        ");
    }

    #[test]
    fn renders_the_unchanged_summary() {
        assert_eq!(
            render_unchanged_summary(1, &options(), false),
            "1 environment unchanged"
        );
        assert_eq!(
            render_unchanged_summary(20, &options(), true),
            "20 environments unchanged"
        );
    }

    #[test]
    fn renders_the_failure_heading() {
        assert_eq!(
            render_failure_heading("install", 1, &options()),
            "1 environment failed to install:"
        );
        assert_eq!(
            render_failure_heading("sync", 3, &options()),
            "3 environments failed to sync:"
        );
    }

    /// A quiet run still gets the reason, otherwise a failing name would be
    /// left without a diagnosis.
    #[test]
    fn a_reason_counts_as_a_failure() {
        assert!(EnvReport::failed("broken").is_failure());
        assert!(EnvReport::reason("broken", "× no such package").is_failure());
        assert!(!installed().is_failure());
    }
}
