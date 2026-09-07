//! Desktop window mode: the same `App` in a native window through the iced
//! umbrella crate (winit, softbuffer, the tiny-skia renderer). Used with
//! `--window`, when stdin is not a terminal, or when the terminal did not
//! answer the kitty graphics probe. Git replies and agent jobs arrive through
//! a subscription; everything else is the terminal path minus the terminal.

use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::futures::channel::mpsc as fmpsc;
use iced::futures::{Stream, StreamExt};
use iced::{Subscription, Task};
use iced_core::{keyboard, mouse, window, Event, Font, Size};

use crate::agent::{self, AgentJob, Server};
use crate::git::ops::{self, Command, Worker};
use crate::runtime::Options;
use crate::ui::app::{App, Element, External, Inbox, Message};
use crate::ui::theme::Theme;

const WINDOW: Size = Size::new(1400.0, 900.0);
const TICK_MS: u64 = 500;

struct Desktop {
    app: App,
    worker: Worker,
    _agent: Option<Server>,
    inbox: Receiver,
}

/// The receiving end of the thread channel, handed to the subscription
/// once. Hashes to a constant so iced keeps the one stream alive.
#[derive(Clone)]
struct Receiver(Arc<Mutex<Option<fmpsc::UnboundedReceiver<External>>>>);

impl Hash for Receiver {
    fn hash<H: Hasher>(&self, h: &mut H) {
        "gitgui-inbox".hash(h);
    }
}

pub fn run_window(opts: &Options) -> anyhow::Result<i32> {
    let opts = opts.clone();
    iced::application(move || boot(&opts), update, view)
        .title("gitgui")
        .theme(|d: &Desktop| d.app.theme.iced())
        .subscription(subscription)
        .default_font(Font::with_name("Fira Sans"))
        .window(window::Settings {
            size: WINDOW,
            icon: crate::ui::logo::pixels().and_then(|p| window::icon::from_rgba(p.rgba.clone(), p.width, p.height).ok()),
            ..window::Settings::default()
        })
        .antialiasing(true)
        .run()
        .map_err(|e| anyhow::anyhow!("desktop window: {e}"))?;
    Ok(0)
}

fn boot(opts: &Options) -> (Desktop, Task<Message>) {
    let (tx, rx) = fmpsc::unbounded::<External>();
    let git_tx = tx.clone();
    let worker = ops::spawn(opts.path.clone(), move |r| {
        let _ = git_tx.unbounded_send(External::Reply(r));
    });
    let (job_tx, job_rx) = std::sync::mpsc::channel::<AgentJob>();
    let agent = match Server::bind(&opts.path, job_tx) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("gitgui: agent socket: {e:#}");
            None
        }
    };
    std::thread::Builder::new()
        .name("agent-bridge".into())
        .spawn(move || {
            while let Ok(job) = job_rx.recv() {
                if tx.unbounded_send(External::Agent(job)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn agent bridge");
    let mut app = App::new(Theme::dark(), "window", 1.0, opts.path.clone());
    app.editor_cmd = opts.editor.clone();
    app.open_on_start = opts.open.clone();
    app.window = WINDOW;
    let desktop = Desktop {
        app,
        worker,
        _agent: agent,
        inbox: Receiver(Arc::new(Mutex::new(Some(rx)))),
    };
    (desktop, Task::none())
}

fn update(d: &mut Desktop, msg: Message) -> Task<Message> {
    match msg {
        Message::External(inbox) => match inbox.take() {
            Some(External::Reply(r)) => d.app.apply(r),
            Some(External::Agent(job)) => {
                let mut screenshot = None;
                let resp = agent::handle_in_app(&mut d.app, job.request, &mut screenshot);
                let resp = if screenshot.is_some() {
                    agent::err("screenshot is not available in window mode")
                } else {
                    resp
                };
                let _ = job.reply.send(resp);
            }
            None => {}
        },
        Message::Tick => {
            d.app.toasts_active();
            d.app.flush_state();
        }
        m => d.app.update(m),
    }
    for cmd in d.app.pending.drain(..) {
        let _ = d.worker.tx.send(cmd);
    }
    let mut tasks: Vec<Task<Message>> = Vec::new();
    for op in d.app.ops.drain(..) {
        tasks.push(iced_runtime::task::widget(op).map(|_| Message::Nothing));
    }
    for text in d.app.pending_copy.drain(..) {
        tasks.push(iced::clipboard::write(text));
    }
    if d.app.quit {
        d.app.flush_state();
        let _ = d.worker.tx.send(Command::Quit);
        tasks.push(iced::exit());
    }
    Task::batch(tasks)
}

fn view(d: &Desktop) -> Element<'_> {
    d.app.view()
}

fn subscription(d: &Desktop) -> Subscription<Message> {
    let mut subs = vec![
        iced::keyboard::listen().map(|e| match e {
            keyboard::Event::KeyPressed { key, modifiers, .. } => Message::Key(key, modifiers),
            _ => Message::Nothing,
        }),
        iced::event::listen_with(|ev, _status, _id| match ev {
            Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
            Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::CursorMoved(position)),
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::ModifiersChanged(m)),
            _ => None,
        }),
        Subscription::run_with(d.inbox.clone(), inbox_stream),
    ];
    if !d.app.toasts.is_empty() || d.app.state_dirty() {
        subs.push(Subscription::run_with(TICK_MS, ticker));
    }
    Subscription::batch(subs)
}

type BoxStream = Pin<Box<dyn Stream<Item = Message> + Send>>;

fn inbox_stream(rx: &Receiver) -> BoxStream {
    match rx.0.lock().ok().and_then(|mut g| g.take()) {
        Some(rx) => Box::pin(rx.map(|e| Message::External(Inbox::new(e)))),
        None => Box::pin(iced::futures::stream::pending()),
    }
}

/// A tick every `ms` from a plain thread; the thread ends with the stream.
fn ticker(ms: &u64) -> BoxStream {
    let (tx, rx) = fmpsc::unbounded::<()>();
    let period = Duration::from_millis(*ms);
    std::thread::spawn(move || loop {
        std::thread::sleep(period);
        if tx.unbounded_send(()).is_err() {
            break;
        }
    });
    Box::pin(rx.map(|_| Message::Tick))
}
