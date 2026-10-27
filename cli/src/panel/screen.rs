// The panel on the terminal, on a thread of its own. It never holds the panel's lock while it
// draws, so no lane waits on it. Whatever ends it, a panic, a terminal that fails, or
// `listen` stopping or restarting onto a new build, leaves the terminal as it was (`down`),
// and the log goes back to stdout.

use std::cell::Cell;
use std::io::{self, Stdout, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::{Once, TryLockError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::draw::{Ui, draw};
use super::view::{self, Live};
use super::{Logged, log, quit, set_on, state};
use crate::gateway;
use crate::health::Health;
use crate::prebuilt;
use crate::runtime::Shared;
use crate::time::{local_clock, now_ms};
use crate::trace::{self, Mark};
use crate::update::Update;

const TICK: Duration = Duration::from_millis(250);
// The log lines copied for a draw: more than any terminal is tall.
const SHOWN: usize = 500;

// Whether this process's terminal shows the panel, and whether this thread is the panel's own:
// the panic hook reads them, which is passed nothing.
static RAISED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static UI: Cell<bool> = const { Cell::new(false) };
}

// The panel while it shows: dropped, it stops and the terminal is restored.
pub struct Screen {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        down();
    }
}

// None when the terminal can't take it: the log stays plain.
pub fn start(update: Option<Update>, shared: Arc<Shared>) -> Option<Screen> {
    let raised = enable_raw_mode().is_ok() && {
        RAISED.store(true, Ordering::SeqCst);
        execute!(io::stdout(), EnterAlternateScreen, Hide).is_ok()
    };
    let terminal = raised.then(|| Terminal::new(CrosstermBackend::new(io::stdout())).ok()).flatten();
    let Some(terminal) = terminal else {
        down();
        return None;
    };
    hook();
    trace::sink(log);
    set_on(true);
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        UI.with(|ui| ui.set(true));
        let _ = catch_unwind(AssertUnwindSafe(|| run(terminal, &stopping, update.as_ref(), &shared.health)));
        down();
    });
    Some(Screen { stop, thread: Some(thread) })
}

// The terminal as it was, then the log back on stdout.
fn down() {
    restore();
    if super::on() {
        replay(&mut state());
    }
}

// Once: raw mode off, the alternate screen left, the cursor shown.
fn restore() {
    if RAISED.swap(false, Ordering::SeqCst) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
    }
}

// What the panel kept of the log, onto stdout, then the log goes there itself: under the
// panel's lock, so a line logged meanwhile waits and comes after (`panel::log`).
fn replay(state: &mut super::State) {
    let mut out = io::stdout().lock();
    for line in std::mem::take(&mut state.log) {
        let _ = writeln!(out, "{}", trace::plain(&line));
    }
    let _ = out.flush();
    set_on(false);
}

// A panic's message would draw over the panel: one on a lane's or the watcher's thread, which
// goes on, is a line in the log pane, as stderr would have had it. One on the panel's own
// thread or the main one restores the terminal first, then says itself as ever. The hook
// never waits on the panel's lock, which may be the panicking thread's own: a lane's line is
// dropped while it stays held, and the log is replayed once the panel's thread or the main
// one has unwound (`down`).
fn hook() {
    // The panic hook is the process's: set once.
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let main = std::thread::current().name() == Some("main");
            if super::on() && !main && !UI.with(Cell::get) {
                for _ in 0..HOOK_TRIES {
                    let mut state = match super::STATE.try_lock() {
                        Ok(state) => state,
                        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                        Err(TryLockError::WouldBlock) => {
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                    };
                    // Gone down meanwhile: said on stderr, as ever.
                    if !super::on() {
                        break;
                    }
                    super::log_into(&mut state, Logged { clock: local_clock(), mark: Mark::Error, lead: "·".into(), issue: false, text: info.to_string().replace('\n', " ") });
                    return;
                }
                if super::on() {
                    return;
                }
            }
            restore();
            if super::on()
                && let Ok(mut state) = super::STATE.try_lock()
            {
                replay(&mut state);
            }
            before(info);
        }));
    });
}

const HOOK_TRIES: usize = 50;

fn run(mut terminal: Terminal<CrosstermBackend<Stdout>>, stop: &AtomicBool, update: Option<&Update>, health: &Health) -> io::Result<()> {
    let (mut ui, started) = (Ui::default(), Instant::now());
    // Ends too once the terminal is restored, by a panic on another thread.
    while !stop.load(Ordering::Relaxed) && RAISED.load(Ordering::SeqCst) {
        let (view, log) = {
            let live = live(update, health);
            let state = state();
            let lines: Vec<Logged> = state.log.iter().rev().filter(|l| ui.opened.as_ref().is_none_or(|issue| l.issue && &l.lead == issue)).take(SHOWN).cloned().collect();
            (view::view(&state, &live, Instant::now()), lines.into_iter().rev().collect::<Vec<_>>())
        };
        ui.picked = ui.picked.min(view.slots.len().saturating_sub(1));
        terminal.draw(|frame| draw(frame, &view, &log, &ui, started))?;
        if !event::poll(TICK)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => pressed(key, &mut ui, &view),
            Event::Resize(..) => terminal.autoresize()?,
            _ => {}
        }
    }
    Ok(())
}

fn pressed(key: KeyEvent, ui: &mut Ui, view: &view::View) {
    match key.code {
        // Raw mode takes Ctrl+C as a key: it still ends `listen` at once, as it always has.
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            down();
            std::process::exit(130);
        }
        KeyCode::Char('q') => quit(),
        KeyCode::Char('l') => ui.log_only = !ui.log_only,
        KeyCode::Up | KeyCode::Char('k') => ui.picked = ui.picked.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => ui.picked = (ui.picked + 1).min(view.slots.len().saturating_sub(1)),
        KeyCode::Enter if ui.opened.is_some() => ui.opened = None,
        KeyCode::Enter => ui.opened = view.slots.get(ui.picked).and_then(|s| s.issue.clone()),
        KeyCode::Esc => ui.opened = None,
        _ => {}
    }
}

// Read as it draws: the budgets, the silent hosts, and the update.
fn live(update: Option<&Update>, health: &Health) -> Live {
    let now = now_ms();
    Live { budgets: gateway::shown(u64::try_from(now).unwrap_or(0)), silences: health.silences(), commit: prebuilt::commit().map(String::from), building: update.and_then(Update::building), draining: update.is_some_and(Update::draining) }
}
