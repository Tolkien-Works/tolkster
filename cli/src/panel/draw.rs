// The panel laid out and coloured, from its rows (view.rs). The meaning of each row is in
// its words and signs, as the log's is: colour only adds to them.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::Logged;
use super::frame::{Checks, MergeState, Pr, TriageState};
use crate::machine::Outcome;
use super::view::{Agent, Also, Budget, Link, Need, SlotRow, View};
use crate::gateway::is_low;
use crate::trace::{Mark, STUCK_AFTER};

const AMBER: Color = Color::Yellow;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
// A job's spinner turns amber past this, and red near STUCK_AFTER.
const SLOW_JOB: Duration = Duration::from_secs(2 * 60);
// A slot row's last step, padded to this.
const STEP: usize = 31;
const GAUGE: usize = 20;

// What the person looks at: the panel or the log alone, and the slot picked, whose issue's
// lines the log shows alone once opened.
#[derive(Default)]
pub struct Ui {
    pub log_only: bool,
    pub picked: usize,
    pub opened: Option<String>,
}

pub fn draw(frame: &mut Frame, view: &View, log: &[Logged], ui: &Ui, started: Instant) {
    let area = frame.area();
    let needs = needs(view);
    let [strip, rest] = Layout::vertical([Constraint::Length(u16::from(needs.is_some())), Constraint::Min(0)]).areas(area);
    if let Some(needs) = needs {
        frame.render_widget(Paragraph::new(needs).style(Style::new().fg(Color::Black).bg(AMBER)), strip);
    }
    let log_area = if ui.log_only {
        rest
    } else {
        let lines = panel(view, ui, started, usize::from(rest.width.saturating_sub(2)));
        let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(rest.height.saturating_sub(3));
        let [top, bottom] = Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).areas(rest);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title_top(Line::from(" tolkster listen "))
            .title_top(link(view).right_aligned())
            .title_bottom(footer(view));
        frame.render_widget(Paragraph::new(lines).block(block), top);
        bottom
    };
    draw_log(frame, log, ui, log_area);
}

fn needs(view: &View) -> Option<Line<'static>> {
    if view.needs.is_empty() {
        return None;
    }
    let said: Vec<String> = view.needs.iter().map(|need| match need {
        Need::Waiting(issue) => format!("{issue} waits at a prompt"),
        Need::Stalled(issue) => format!("{issue} stalled"),
        Need::NoRounds(issue) => format!("{issue} fails its checks with no round left"),
        Need::Anomaly(issue) => format!("{issue} holds a slot though its issue is done"),
        Need::Silent(host) => format!("{host} isn't answering"),
        Need::Refused(host) => format!("{host}'s tolkster can't read its config: rejoin it with host add"),
        Need::Spent(api) => format!("{api}'s budget is spent"),
    }).collect();
    Some(Line::from(format!(" needs you: {}", said.join(" · "))).style(Style::new().add_modifier(Modifier::BOLD)))
}

fn link(view: &View) -> Line<'static> {
    let (colour, said) = match view.link {
        Link::Connecting => (AMBER, "connecting".to_string()),
        Link::Connected(age) => (Color::Green, format!("connected {}", ago(age))),
        Link::Reconnecting(left) => (AMBER, format!("reconnecting in {}", ago(left))),
        Link::Superseded => (Color::Red, "superseded".into()),
        Link::Stopped => (Color::Red, "stopped".into()),
    };
    let mut spans = vec![Span::styled("● ", Style::new().fg(colour)), Span::raw(said)];
    if view.reconnects > 0 {
        spans.push(Span::raw(format!(" · {} reconnects", view.reconnects)));
    }
    if let Some(ping) = view.ping {
        spans.push(Span::raw(format!(" · ping {} ms", ping.as_millis())));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

// The hosts are on the slots line, so the footer holds the listing's age alone.
pub(super) fn footer(view: &View) -> Line<'static> {
    match view.listing {
        Some((age, took)) => Line::from(format!(" listing {} ago, took {:.1} s ", ago(age), took.as_secs_f64())),
        None => Line::from(" no listing yet "),
    }
}

// `width` inside the border, which the rules span.
pub(super) fn panel(view: &View, ui: &Ui, started: Instant, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![gauges(view, width)];
    lines.extend(budgets(view, width));
    lines.push(build(view));
    lines.push(rule("Slots", None, width));
    let hosts = view.slots.iter().map(|s| s.host.chars().count()).fold(7, usize::max);
    // The PR column is as wide as the widest PR, so a slot without one keeps the timer's place.
    let prs = view.slots.iter().filter_map(|s| s.pr.as_ref()).map(|pr| pr_span(pr).width()).max().unwrap_or(0);
    let words = view.slots.iter().map(|s| slot_agent(s).2.chars().count()).fold(11, usize::max);
    for (n, slot) in view.slots.iter().enumerate() {
        let (dot, colour, word) = slot_agent(slot);
        let mut spans = vec![
            Span::raw(if n == ui.picked { "▸ " } else { "  " }),
            Span::styled(format!("{dot} "), Style::new().fg(colour)),
            Span::styled(format!("{:<8} ", slot.issue.as_deref().unwrap_or("·")), Style::new().fg(Color::Cyan)),
            Span::raw(format!("{:<hosts$} ", slot.host)),
            Span::styled(format!("{word:<words$} "), Style::new().fg(colour)),
            Span::raw(format!("{:>5}  ", ago(slot.for_))),
        ];
        let mut rest = Vec::new();
        if slot.anomaly {
            rest.push(Span::styled("⚠ issue done   ", Style::new().fg(Color::Red)));
        }
        let pr = slot.pr.as_ref().map(pr_span).unwrap_or_default();
        let pad = " ".repeat(prs - pr.width());
        rest.push(pr);
        rest.push(Span::raw(pad));
        if let Some((what, left)) = &slot.deadline {
            rest.push(Span::styled(format!("⏲ {what} in {}   ", ago(*left)), Style::new().fg(Color::DarkGray)));
        }
        if let Some((label, for_)) = &slot.job {
            rest.extend(job(label, *for_, started));
        }
        let (step, colour) = match &slot.last {
            Some((command, outcome, before)) => {
                let (said, colour) = match outcome {
                    Outcome::Ok => ("ok", Color::Reset),
                    Outcome::Failed => ("failed", Color::Red),
                    Outcome::Retryable => ("retryable", AMBER),
                    Outcome::Held => ("held", AMBER),
                    Outcome::Gone => ("gone", Color::DarkGray),
                };
                (format!("{command} {said} {}", ago(*before)), colour)
            }
            None => (String::new(), Color::Reset),
        };
        let times = [Span::raw(format!("{:>5}  ", ago(slot.ran))), Span::styled(format!("{step:<STEP$}   "), Style::new().fg(colour))];
        // A narrow terminal loses the run time and last step before the rest.
        if spans.iter().chain(&times).chain(&rest).map(Span::width).sum::<usize>() <= width {
            spans.extend(times);
        } else {
            spans.push(Span::raw(" "));
        }
        spans.extend(rest);
        lines.push(Line::from(spans));
        if !slot.waiting.is_empty() {
            let verb = if slot.waiting.len() == 1 { "waits" } else { "wait" };
            lines.push(Line::from(Span::styled(format!("      └ {} {verb} on this", slot.waiting.join(", ")), Style::new().fg(AMBER))));
        }
    }
    for _ in 0..view.free {
        lines.push(Line::from(vec![Span::raw("  "), Span::styled("· ─────    free", Style::new().fg(Color::DarkGray))]));
    }
    if !view.queued.is_empty() {
        lines.push(rule("Queued", (!view.framed).then_some("may be stale"), width));
        for row in &view.queued {
            let style = match row.age.as_secs() {
                age if age < 5 * 60 => Style::new().fg(AMBER),
                age if age < 30 * 60 => Style::new().fg(AMBER).add_modifier(Modifier::DIM),
                _ => Style::new().fg(Color::DarkGray),
            };
            let said = if view.framed { "since" } else { "said" };
            let through = row.through.as_ref().map(|root| format!(" · waits on you through {root}")).unwrap_or_default();
            lines.push(Line::from(format!("  ⧗ {:<8} {:<50} {said} {}", row.issue, format!("{}{through}", row.reason), ago(row.age))).style(style));
        }
    }
    if !view.parked.is_empty() {
        lines.push(rule("Parked", None, width));
        for row in &view.parked {
            let mut spans = vec![Span::raw(format!("  ‖ {:<8} {:<8} ", row.issue, if row.stalled { "stalled" } else { row.work.name() }))];
            spans.extend(row.pr.as_ref().map(pr_span));
            if row.owner {
                spans.push(Span::styled("waits on you", Style::new().fg(AMBER)));
            }
            lines.push(Line::from(spans));
        }
    }
    if !view.also.is_empty() {
        lines.push(rule("Also running", None, width));
        let mut spans = vec![Span::raw("  ")];
        for also in &view.also {
            match also {
                Also::Job { issue, label, for_ } => {
                    let (sign, colour) = spinner(*for_, started);
                    spans.push(Span::styled(format!("{sign} "), Style::new().fg(colour)));
                    spans.push(Span::raw(format!("{}{label} {}", issue.as_ref().map(|i| format!("{i} ")).unwrap_or_default(), ago(*for_))));
                }
                Also::Held { issue, label, left } => spans.push(Span::raw(format!("⏸ {}{label} held, again in {}", issue.as_ref().map(|i| format!("{i} ")).unwrap_or_default(), ago(*left)))),
            }
            spans.push(Span::raw("   "));
        }
        lines.push(Line::from(spans));
    }
    let mut more: Vec<String> = view.chains.iter().map(|c| format!("{} chain {}/{} answered", c.issue, c.answered, c.issues)).collect();
    more.extend(view.machines.iter().map(|(repo, why)| format!("{repo} no machine: {}", why.said())));
    if let Some((state, due, failures)) = view.triage {
        let state = match state {
            TriageState::Idle => "idle",
            TriageState::Starting => "starting",
            TriageState::Running => "running",
            TriageState::Retrying => "retrying",
        };
        let due = due.map(|d| format!(" in {}", ago(d))).unwrap_or_default();
        let failures = if failures > 0 { format!(" after {failures} failed") } else { String::new() };
        more.push(format!("triage {state}{due}{failures}"));
    }
    // The last hour's line is always there, so the heading is too.
    lines.push(rule("Activity", None, width));
    if !more.is_empty() {
        lines.push(Line::from(whole_items(&more, width)));
    }
    if !view.conversations.is_empty() {
        let said: Vec<String> = view.conversations.iter().map(|(issue, for_)| format!("{issue} {}", ago(*for_))).collect();
        lines.push(Line::from(format!("  conversations  {}", said.join("   "))));
    }
    let o = &view.outcomes;
    // A zero count is dim, so a failure stands out by more than its colour.
    let count = |n: usize, said: &str, colour: Color| Span::styled(format!("{n} {said}"), if n == 0 { Style::new().add_modifier(Modifier::DIM) } else { Style::new().fg(colour) });
    let mut spans = vec![Span::raw("  last hour  "), count(o.ok, "ok", Color::Reset), Span::raw("  "), count(o.failed, "failed", Color::Red), Span::raw("  "), count(o.retryable, "retryable", Color::Reset)];
    for (issue, command, kind) in &o.failures {
        spans.push(Span::styled(format!("   ✖ {issue} {command} ({kind})"), Style::new().fg(Color::Red)));
    }
    lines.push(Line::from(spans));
    lines
}

// `items` on a line, as many whole as `width` holds, then `+N more` for the rest.
fn whole_items(items: &[String], width: usize) -> String {
    const SEP: &str = "   ";
    let said = |shown: usize| {
        let mut said = format!("  {}", items[..shown].join(SEP));
        if shown < items.len() {
            said.push_str(&format!("{}+{} more", if shown > 0 { SEP } else { "" }, items.len() - shown));
        }
        said
    };
    let shown = (0..=items.len()).rev().find(|n| said(*n).chars().count() <= width).unwrap_or(0);
    said(shown)
}

// A label's column, for the panel's head.
fn label(label: &str) -> Span<'static> {
    Span::raw(format!("  {label:<9}"))
}

// The first of `forms` that fits `width`, else the last.
fn fit(forms: impl IntoIterator<Item = Vec<Span<'static>>>, width: usize) -> Line<'static> {
    let mut last = Vec::new();
    for spans in forms {
        if spans.iter().map(Span::width).sum::<usize>() <= width {
            return Line::from(spans);
        }
        last = spans;
    }
    Line::from(last)
}

// Each host's count and whether it answers; narrow, without its bar and what the mark adds.
fn gauges(view: &View, width: usize) -> Line<'static> {
    let used = view.slots.len();
    let cap = view.cap.and_then(|c| usize::try_from(c).ok()).unwrap_or(0);
    let full = cap > 0 && used >= cap;
    // One host's bar would repeat the total's.
    let several = view.hosts.iter().filter(|h| !h.asleep && h.used > 0).count() > 1;
    let line = |bars: bool, said: bool| {
        let mut spans = vec![label("slots"), Span::styled(bar(used, cap, GAUGE), Style::new().fg(if full { AMBER } else { Color::Green })), Span::raw(format!(" {:<5}", format!("{used}/{cap}")))];
        for host in &view.hosts {
            if host.asleep {
                spans.push(Span::raw(format!("  {} ", host.name)));
                spans.push(Span::styled("asleep", Style::new().fg(Color::DarkGray)));
                continue;
            }
            let (used, cap) = (usize::try_from(host.used).unwrap_or(0), usize::try_from(host.cap).unwrap_or(0));
            let colour = if !host.answering || host.refused { Color::Red } else if cap > 0 && used >= cap || used > cap { AMBER } else { Color::Green };
            spans.push(Span::raw(format!("  {} ", host.name)));
            if bars {
                spans.push(Span::styled(bar(used, cap, cap.min(GAUGE)), Style::new().fg(colour)));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::raw(format!("{used}/{cap}{}", if used > cap { " over" } else { "" })));
            let (sign, colour) = if host.answering && !host.refused { ("✔", Color::Green) } else { ("✖", Color::Red) };
            spans.push(Span::styled(format!(" {sign}"), Style::new().fg(colour)));
            if said {
                if host.refused {
                    spans.push(Span::raw(" can't read config"));
                }
                if let Some(silent) = host.silent_for {
                    spans.push(Span::raw(format!(" silent {}", ago(silent))));
                }
                if let Some(asked) = host.asked_in {
                    spans.push(Span::raw(format!(", asked in {}", ago(asked))));
                }
            }
        }
        spans
    };
    fit([line(several, true), line(several, false), line(false, false)], width)
}

// One a line, its name, bar, count and reset each in a column; narrow, without the reset.
fn budgets(view: &View, width: usize) -> Vec<Line<'static>> {
    if view.budgets.is_empty() {
        return vec![Line::from(vec![label("budgets"), Span::styled("not said yet", Style::new().fg(Color::DarkGray))])];
    }
    view.budgets.iter().enumerate().map(|(n, Budget { api, limit, remaining, resets, spent })| {
        let colour = match limit {
            _ if *spent => Color::Red,
            Some(limit) if is_low(*limit, *remaining) => AMBER,
            _ => Color::Green,
        };
        let gauge = limit.map_or_else(|| " ".repeat(5), |limit| bar(usize::try_from(*remaining).unwrap_or(0), usize::try_from(limit).unwrap_or(0), 5));
        let spans = vec![
            label(if n == 0 { "budgets" } else { "" }),
            Span::raw(format!("{api:<7}")),
            Span::styled(gauge, Style::new().fg(colour)),
            Span::styled(format!(" {:<11}", format!("{} left", count(*remaining))), Style::new().fg(colour)),
        ];
        let mut reset = spans.clone();
        reset.push(Span::raw(format!("resets {}", ago(*resets))));
        fit([reset, spans], width)
    }).collect()
}

fn build(view: &View) -> Line<'static> {
    let u = &view.update;
    let mut build = u.commit.as_deref().map_or("unstamped", short).to_string();
    match (u.behind, &u.base) {
        (Some(true), Some(base)) => build.push_str(&format!(", main at {}", short(base))),
        (Some(false), _) => build.push_str(", current"),
        _ => {}
    }
    if let Some(building) = u.building {
        build.push_str(&format!(", building {}", ago(building)));
    }
    let mut spans = vec![label("build"), Span::raw(build)];
    if u.draining {
        spans.push(Span::styled(", draining", Style::new().fg(AMBER)));
    }
    Line::from(spans)
}

fn draw_log(frame: &mut Frame, log: &[Logged], ui: &Ui, area: Rect) {
    let (area, lines) = match &ui.opened {
        Some(issue) => {
            let block = Block::bordered().border_type(BorderType::Rounded).title_top(Line::from(format!(" {issue}'s lines · Enter or Esc closes ")));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            (inner, log.iter().filter(|l| l.issue && &l.lead == issue).collect::<Vec<_>>())
        }
        None => (area, log.iter().collect()),
    };
    let shown = lines.len().saturating_sub(usize::from(area.height));
    let lines: Vec<Line> = lines[shown..].iter().map(|line| logged(line)).collect();
    frame.render_widget(Paragraph::new(lines), area);
}

// As `trace` writes it, in colour of the panel's choosing.
fn logged(line: &Logged) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{}  ", line.clock), Style::new().fg(Color::DarkGray))];
    let width = match line.mark {
        Mark::Routine | Mark::Quiet => 8,
        mark => {
            let (sign, colour) = match mark {
                Mark::Error => ("✖", Color::Red),
                Mark::Warning => ("!", AMBER),
                _ => ("✔", Color::Green),
            };
            spans.push(Span::styled(format!("{sign} "), Style::new().fg(colour)));
            6
        }
    };
    let pad = " ".repeat(width - line.lead.chars().count().min(width));
    // A routine line that changed nothing is dim, so a state change stands out.
    let quiet = line.mark == Mark::Quiet;
    let dim = |style: Style| if quiet { style.add_modifier(Modifier::DIM) } else { style };
    spans.push(Span::styled(line.lead.clone(), dim(if line.issue { Style::new().fg(Color::Cyan) } else { Style::new() })));
    spans.push(Span::styled(format!("{pad} {}", line.text), dim(Style::new().fg(if quiet { Color::DarkGray } else { Color::Reset }))));
    Line::from(spans)
}

// A merged slot's worker is closing: idle says so, dim; a busy one keeps its marker.
fn slot_agent(slot: &SlotRow) -> (&'static str, Color, String) {
    let (dot, colour, word) = agent(slot.agent);
    match (slot.merged, slot.agent) {
        (true, Agent::Idle) => (dot, Color::DarkGray, "merged, closing".into()),
        (true, _) => (dot, colour, format!("{word}, merged")),
        _ => (dot, colour, word.into()),
    }
}

fn agent(agent: Agent) -> (&'static str, Color, &'static str) {
    match agent {
        Agent::Working => ("●", Color::Green, "working"),
        Agent::Background => ("●", Color::Green, "working (background)"),
        Agent::Waiting => ("◐", AMBER, "waiting"),
        Agent::Idle => ("○", Color::Cyan, "idle"),
        Agent::Reserved => ("◌", Color::DarkGray, "reserved"),
        Agent::Launching => ("◌", Color::DarkGray, "launching"),
        Agent::Unseen => ("◌", Color::DarkGray, "unseen"),
        Agent::Silent => ("◌", Color::Red, "silent"),
        Agent::HostSilent => ("◌", Color::Red, "host silent"),
    }
}

// `#7 CI failed · conflicts · 2 rounds`, red when checks fail, amber while they run.
fn pr_span(pr: &Pr) -> Span<'static> {
    let (checks, colour) = match pr.checks {
        Checks::Passed => ("CI passed", Color::Green),
        Checks::Failed => ("CI failed", Color::Red),
        Checks::CouldntRun => ("CI couldn't run", Color::Red),
        Checks::Pending => ("CI running", AMBER),
    };
    let merge = match pr.merge_state {
        Some(MergeState::Behind) => " · behind",
        Some(MergeState::Dirty) => " · conflicts",
        _ => "",
    };
    let rounds = match pr.rounds {
        0 => String::new(),
        1 => " · 1 round".into(),
        n => format!(" · {n} rounds"),
    };
    Span::styled(format!("#{} {checks}{merge}{rounds}   ", pr.number), Style::new().fg(colour))
}

fn job(label: &str, for_: Duration, started: Instant) -> Vec<Span<'static>> {
    let (sign, colour) = spinner(for_, started);
    vec![Span::styled(format!("{sign} "), Style::new().fg(colour)), Span::raw(format!("{label} {}", ago(for_)))]
}

fn spinner(for_: Duration, started: Instant) -> (&'static str, Color) {
    let frame = SPINNER[usize::try_from(started.elapsed().as_millis() / 100).unwrap_or(0) % SPINNER.len()];
    let colour = match for_ {
        f if f >= STUCK_AFTER.mul_f32(0.8) => Color::Red,
        f if f >= SLOW_JOB => AMBER,
        _ => Color::Reset,
    };
    (frame, colour)
}

fn rule(title: &str, right: Option<&str>, width: usize) -> Line<'static> {
    let right = right.map(|r| format!(" {r} ─────")).unwrap_or_default();
    let fill = width.saturating_sub(title.chars().count() + right.chars().count() + 3);
    Line::from(format!("─ {title} {}{right}", "─".repeat(fill))).style(Style::new().fg(Color::DarkGray))
}

fn bar(filled: usize, of: usize, width: usize) -> String {
    let on = if of == 0 { 0 } else { (filled.min(of) * width).div_ceil(of) };
    format!("{}{}", "█".repeat(on), "░".repeat(width - on.min(width)))
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn count(n: u64) -> String {
    if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}

// `41s`, `12m`, `2h14m`.
pub fn ago(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h{:02}m", s / 3600, s / 60 % 60),
    }
}
