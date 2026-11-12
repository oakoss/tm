//! The terminal `tm tui` runs in: the loop between keys, and handing the
//! terminal to an editor or a shell and taking it back.

use std::io::{self, Write as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, enable_raw_mode};
#[cfg(unix)]
use signal_hook::consts::{SIGCONT, SIGSTOP, SIGTSTP};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::{SigId, flag, low_level};
use tm_store::Store;

use crate::{Actions, Flow, Seen, Shell, editor};

/// How often a write elsewhere is looked for between keys (ADR 0018).
const POLL: Duration = Duration::from_millis(250);

/// Runs the TUI over `store` until a key quits it. Between keys `look` finds
/// writes elsewhere, reopening `store` if it must, as for a replaced
/// Snapshot; its error ends the TUI. The terminal is restored on return, and
/// on panic even where a panic aborts.
///
/// # Errors
///
/// A terminal failure, starting it included, or `look`'s error.
pub fn run<E: From<io::Error>>(
    place: impl Into<String>,
    store: Store,
    actions: &dyn Actions,
    look: impl FnMut(&mut Store) -> Result<Seen, E>,
) -> Result<Ended, E> {
    // Ratatui's own hook leaves the alternate screen and raw mode but not
    // the hidden cursor, which only dropping the terminal shows.
    let shown = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), Show);
        shown(info);
    }));
    // A signal that ends the process would skip the restore below, so the
    // first is caught and the loop leaves through it. A second, or one after
    // the TUI returns, acts as it would have. SIGHUP is left alone: the
    // terminal is gone, and caught it would leave input polling spinning.
    let mut signals = Signals {
        caught: Arc::new(AtomicUsize::new(0)),
        default: Arc::new(AtomicBool::new(false)),
        interrupt: Vec::new(),
        stop: Arc::new(AtomicBool::new(false)),
        resumed: Arc::new(AtomicBool::new(false)),
        editing: Arc::new(AtomicBool::new(false)),
    };
    signals.catch(SIGTERM)?;
    signals.listen()?;
    signals.control_jobs()?;
    let mut shell = Shell::new(place);
    shell.load(&store, actions);
    let ran = match ratatui::try_init() {
        Ok(mut terminal) => {
            let ran = drive(
                &mut terminal,
                &mut shell,
                (store, actions),
                look,
                &mut signals,
            );
            drop(terminal);
            ran
        }
        // Raw mode may be on already.
        Err(err) => Err(err.into()),
    };
    ratatui::restore();
    signals.default.store(true, Ordering::Relaxed);
    // Written, not `eprintln!`, which panics on a broken stderr pipe and
    // would lose `ran`.
    let mut stderr = io::stderr().lock();
    for (noun, draft) in shell.kept_drafts() {
        let _ = writeln!(
            stderr,
            "tm: a {noun} not written is kept in {}",
            draft.display()
        );
    }
    ran
}

/// The signals that end the TUI: the last one caught, and whether the next
/// takes its default action.
struct Signals {
    caught: Arc<AtomicUsize>,
    default: Arc<AtomicBool>,
    /// SIGINT's handlers, dropped while an editor has the terminal.
    interrupt: Vec<SigId>,
    stop: Arc<AtomicBool>,
    resumed: Arc<AtomicBool>,
    /// While an editor has the terminal a SIGTSTP stops the TUI at once,
    /// with the editor, as a shell expects of its whole job.
    editing: Arc<AtomicBool>,
}

impl Signals {
    fn control_jobs(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            flag::register_conditional_default(SIGTSTP, Arc::clone(&self.editing))?;
            flag::register(SIGTSTP, Arc::clone(&self.stop))?;
            flag::register(SIGCONT, Arc::clone(&self.resumed))?;
        }
        Ok(())
    }

    fn catch(&self, signal: i32) -> io::Result<Vec<SigId>> {
        let value = usize::try_from(signal).unwrap_or(usize::MAX);
        Ok(vec![
            flag::register_conditional_default(signal, Arc::clone(&self.default))?,
            flag::register_usize(signal, Arc::clone(&self.caught), value)?,
            flag::register(signal, Arc::clone(&self.default))?,
        ])
    }

    /// Takes SIGINT back after an editor; the terminal is taken back too,
    /// so a stop and resume while it ran need nothing more.
    fn listen(&mut self) -> io::Result<()> {
        self.interrupt = self.catch(SIGINT)?;
        self.editing.store(false, Ordering::Relaxed);
        self.settle();
        Ok(())
    }

    /// Whether a signal asked the TUI to end.
    fn ending(&self) -> bool {
        self.caught.load(Ordering::Relaxed) != 0
    }

    /// Forgets a stop or resume the terminal has been taken back since.
    fn settle(&self) {
        self.stop.store(false, Ordering::Relaxed);
        self.resumed.store(false, Ordering::Relaxed);
    }

    /// Leaves SIGINT to an editor, as git does: Ctrl-C there reaches this
    /// process too, as one of its group, and is the editor's to act on.
    fn hold(&mut self) {
        self.editing.store(job_control(), Ordering::Relaxed);
        for id in self.interrupt.drain(..) {
            low_level::unregister(id);
        }
    }
}

/// Whether a shell with job control started this process's group, so one
/// can bring it back after it stops: its parent is in this session but
/// another group. Without one, as in `tmux new-window 'tm tui'` or `exec tm
/// tui`, a stop would hang the terminal for good.
#[cfg(unix)]
fn job_control() -> bool {
    use rustix::process::{getpgid, getpgrp, getppid, getsid};
    let Some(parent) = getppid() else {
        return false;
    };
    let ours = (getpgrp(), getsid(None));
    let theirs = (getpgid(Some(parent)), getsid(Some(parent)));
    matches!((ours, theirs), ((group, Ok(session)), (Ok(their_group), Ok(their_session)))
        if their_group != group && their_session == session)
}

#[cfg(not(unix))]
fn job_control() -> bool {
    false
}

/// Waits, stopped, until this process's group has the terminal again, as
/// taking it from the background would stop or fail it: after `bg`, until
/// `fg`. False when a signal ends the TUI first, as a shell's `kill %1`
/// sends SIGTERM then SIGCONT, whose SIGTERM another thread may handle a
/// moment after this one wakes.
fn back_in_front(signals: &Signals) -> io::Result<bool> {
    // Only another group named in front waits; an error, as with no
    // controlling terminal, goes on to take it, and fails there if it must.
    #[cfg(unix)]
    let behind = || {
        matches!(rustix::termios::tcgetpgrp(io::stdin()),
            Ok(front) if front != rustix::process::getpgrp())
    };
    #[cfg(unix)]
    loop {
        if signals.ending() {
            return Ok(false);
        }
        if !behind() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
        // `fg` may have come during the sleep.
        if !signals.ending() && behind() {
            low_level::raise(SIGSTOP)?;
        }
    }
    #[cfg(not(unix))]
    Ok(!signals.ending())
}

/// Hands the terminal back and stops until a shell's `fg` sends SIGCONT,
/// then takes it again. False when no shell could bring it back.
fn suspend(terminal: &mut DefaultTerminal, signals: &Signals) -> io::Result<bool> {
    signals.stop.store(false, Ordering::Relaxed);
    if !job_control() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        release()?;
        if !signals.ending() {
            low_level::raise(SIGSTOP)?;
        }
        if back_in_front(signals)? {
            retake(terminal)?;
            signals.settle();
        }
    }
    #[cfg(not(unix))]
    let _ = terminal;
    Ok(true)
}

const NO_SHELL: &str = "Ctrl-Z does nothing here: no shell started tm tui to bring it back";

/// Why the TUI ended without an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Quit,
    /// A signal asked it to stop; its number.
    Signal(i32),
}

/// Runs `run` with the terminal as the TUI found it, then takes it back and
/// redraws it whole.
fn suspended<T>(terminal: &mut DefaultTerminal, run: impl FnOnce() -> T) -> io::Result<T> {
    release()?;
    let ran = run();
    retake(terminal)?;
    Ok(ran)
}

/// The terminal as the TUI found it, the cursor shown.
fn release() -> io::Result<()> {
    ratatui::restore();
    execute!(io::stdout(), Show)
}

/// Raw mode and the alternate screen again, redrawn whole.
fn retake(terminal: &mut DefaultTerminal) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Clear(ClearType::All))?;
    // Both buffers emptied, so the next draw writes every cell. Not
    // `terminal.clear()`, which first asks the terminal where its cursor is
    // and fails where nothing answers.
    terminal.swap_buffers();
    terminal.swap_buffers();
    Ok(())
}

fn drive<E: From<io::Error>>(
    terminal: &mut DefaultTerminal,
    shell: &mut Shell,
    (mut store, actions): (Store, &dyn Actions),
    mut look: impl FnMut(&mut Store) -> Result<Seen, E>,
    signals: &mut Signals,
) -> Result<Ended, E> {
    loop {
        let caught = signals.caught.load(Ordering::Relaxed);
        if caught != 0 {
            return Ok(Ended::Signal(i32::try_from(caught).unwrap_or(i32::MAX)));
        }
        if signals.stop.load(Ordering::Relaxed) {
            if !suspend(terminal, signals)? {
                shell.say(NO_SHELL);
            }
        } else if signals.resumed.swap(false, Ordering::Relaxed) && back_in_front(signals)? {
            // Stopped by a SIGSTOP, which cannot be caught.
            retake(terminal)?;
        }
        terminal.draw(|frame| shell.render(frame))?;
        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
        {
            match shell.key(key) {
                Flow::Quit => return Ok(Ended::Quit),
                Flow::Reload => shell.load(&store, actions),
                Flow::Write => {
                    shell.write(actions);
                    shell.load(&store, actions);
                }
                Flow::Compose => {
                    signals.hold();
                    let (start, draft) = shell.drafting();
                    let draft = editor::Draft {
                        start: &start,
                        ..draft
                    };
                    let composed = suspended(terminal, || editor::compose(draft));
                    signals.listen()?;
                    shell.compose(composed?);
                    shell.write(actions);
                    shell.load(&store, actions);
                }
                Flow::Suspend => {
                    if !suspend(terminal, signals)? {
                        shell.say(NO_SHELL);
                    }
                }
                Flow::Stay => {}
            }
        }
        let seen = look(&mut store)?;
        if matches!(seen, Seen::Changed(_)) {
            shell.load(&store, actions);
        }
        shell.seen(seen);
    }
}
