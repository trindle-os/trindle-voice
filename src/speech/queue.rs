// The speaking queue, priorities, cancelling and the pipeline from words to sound; the voice and the sink are handed in so tests use stand-ins.
// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::phrases::Store;
use super::play::Sink;
use super::reader::NORMAL_SPEED;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Priority {
    Background,
    Normal,
    Focus,
    Urgent,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Priority> {
        Some(match s {
            "urgent" => Priority::Urgent,
            "focus" => Priority::Focus,
            "normal" => Priority::Normal,
            "background" => Priority::Background,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub client: u64,
    pub id: String,
    pub priority: Priority,

    pub voice: String,
    pub rate: u8,
    pub volume: u8,
    pub text: String,
}

pub trait Renderer: Send + Sync {

    fn render(&self, voice: &str, text: &str, speed: f32, cancel: &Arc<AtomicBool>, each: &mut dyn FnMut(Vec<i16>) -> bool) -> Result<u32, String>;
    fn status(&self) -> &'static str;
    fn phrases(&self, voice: &str) -> Option<Arc<Store>>;

    fn warm(&self) {}
}

struct Job {
    req: Request,
    cancel: Arc<AtomicBool>,
    queued: Instant,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    current: Option<(u64, String, Priority, Arc<AtomicBool>)>,
    clients: HashMap<u64, Sender<String>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    paused: AtomicBool,
}

pub struct Engine {
    shared: Arc<Shared>,
    voice: Arc<dyn Renderer>,
}

pub const VOICE_RATE: u32 = crate::RATE;

const MAX_QUEUE: usize = 256;
const BACKGROUND_WAIT: Duration = Duration::from_secs(5);

pub fn speed_of(rate: u8) -> f32 {
    NORMAL_SPEED - 0.4 + f32::from(rate.min(100)) * 0.008
}

pub fn parts(text: &str) -> Vec<(Option<u32>, String)> {
    let mut out = Vec::new();
    let mut marker = None;
    for (i, piece) in text.split('\u{1}').enumerate() {
        if i % 2 == 1 {
            marker = piece.trim().parse::<u32>().ok();
        } else if !piece.trim().is_empty() || marker.is_some() {
            out.push((marker.take(), piece.trim().to_string()));
        }
    }
    out
}

impl Engine {
    pub fn start(voice: Arc<dyn Renderer>, sink: Box<dyn Sink>) -> Engine {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), wake: Condvar::new(), paused: AtomicBool::new(false) });
        let engine = Engine { shared: shared.clone(), voice: voice.clone() };
        std::thread::spawn(move || work(shared, voice, sink));
        engine
    }

    pub fn connect(&self, client: u64, events: Sender<String>) {
        self.shared.state.lock().unwrap_or_else(|p| p.into_inner()).clients.insert(client, events);
    }

    pub fn disconnect(&self, client: u64) {
        self.stop(client);
        self.shared.state.lock().unwrap_or_else(|p| p.into_inner()).clients.remove(&client);
    }

    pub fn warm(&self) {
        self.voice.warm();
    }

    pub fn status(&self) -> &'static str {
        let busy = {
            let s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            s.current.is_some() || !s.queue.is_empty()
        };
        match self.voice.status() {
            "ready" if busy => "busy",
            other => other,
        }
    }

    pub fn submit(&self, req: Request) {
        let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut cancelled = Vec::new();

        let cuts = |p: Priority| match req.priority {
            Priority::Urgent => true,
            Priority::Focus => p != Priority::Urgent,
            Priority::Normal => p == Priority::Background,
            Priority::Background => false,
        };
        let (dropped, kept): (Vec<_>, Vec<_>) = s.queue.drain(..).partition(|j| cuts(j.req.priority));
        s.queue = kept.into();
        for j in dropped {
            cancelled.push((j.req.client, j.req.id));
        }
        if let Some((_, _, p, flag)) = &s.current
            && cuts(*p)
        {
            flag.store(true, Ordering::SeqCst);
        }
        if s.queue.len() >= MAX_QUEUE {
            send(&s, req.client, format!("ERROR id={} reason=queue-full", req.id));
            return;
        }
        s.queue.push_back(Job { req, cancel: Arc::new(AtomicBool::new(false)), queued: Instant::now() });
        for (client, id) in cancelled {
            send(&s, client, format!("CANCELLED id={id}"));
        }
        self.shared.wake.notify_all();
    }

    pub fn stop(&self, client: u64) {
        let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        let (mine, others): (Vec<_>, Vec<_>) = s.queue.drain(..).partition(|j| j.req.client == client);
        s.queue = others.into();
        for j in mine {
            send(&s, client, format!("CANCELLED id={}", j.req.id));
        }
        if let Some((c, _, _, flag)) = &s.current
            && *c == client {
                flag.store(true, Ordering::SeqCst);
            }
        self.shared.wake.notify_all();
    }

    pub fn pause(&self, on: bool) {
        self.shared.paused.store(on, Ordering::SeqCst);
    }
}

fn send(s: &State, client: u64, line: String) {
    if let Some(tx) = s.clients.get(&client) {
        let _ = tx.send(line);
    }
}

fn say(shared: &Shared, client: u64, line: String) {
    send(&shared.state.lock().unwrap_or_else(|p| p.into_inner()), client, line);
}

fn next(shared: &Shared) -> Option<Job> {
    let mut s = shared.state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        let now = Instant::now();
        let mut stale = Vec::new();
        s.queue.retain(|j| {
            let old = j.req.priority == Priority::Background && now.duration_since(j.queued) > BACKGROUND_WAIT;
            if old {
                stale.push((j.req.client, j.req.id.clone()));
            }
            !old
        });
        for (client, id) in stale {
            send(&s, client, format!("CANCELLED id={id}"));
        }
        let best = s.queue.iter().map(|j| j.req.priority).max();
        if let Some(best) = best {
            let at = s.queue.iter().position(|j| j.req.priority == best).unwrap_or(0);
            let job = s.queue.remove(at).expect("position is in range");
            s.current = Some((job.req.client, job.req.id.clone(), job.req.priority, job.cancel.clone()));
            return Some(job);
        }
        let (guard, _) = shared.wake.wait_timeout(s, Duration::from_millis(500)).unwrap_or_else(|p| p.into_inner());
        s = guard;
    }
}

fn work(shared: Arc<Shared>, voice: Arc<dyn Renderer>, mut sink: Box<dyn Sink>) {
    while let Some(job) = next(&shared) {
        let Request { client, id, voice: name, rate, volume, text, .. } = job.req.clone();
        let outcome = speak(&shared, &*voice, &mut *sink, &job, &name, rate, volume, &text);
        let line = match outcome {
            _ if job.cancel.load(Ordering::SeqCst) => format!("CANCELLED id={id}"),
            Ok(()) => format!("DONE id={id}"),
            Err(why) => format!("ERROR id={id} reason={why}"),
        };
        say(&shared, client, line);
        shared.state.lock().unwrap_or_else(|p| p.into_inner()).current = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn speak(shared: &Shared, voice: &dyn Renderer, sink: &mut dyn Sink, job: &Job, name: &str, rate: u8, volume: u8, text: &str) -> Result<(), String> {
    let req = &job.req;
    let mut started = false;
    let begin = |started: &mut bool| announce_start(shared, req, started);
    let speed = speed_of(rate);
    let store = voice.phrases(name);
    for (marker, piece) in parts(text) {
        if job.cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(n) = marker {
            say(shared, req.client, format!("MARKER id={} n={n}", req.id));
        }
        if piece.is_empty() {
            continue;
        }
        if let Some(sound) = store.as_ref().and_then(|s| s.get(&piece).map(|x| (x, s.rate))) {
            let (samples, sample_rate) = sound;
            let stretched = super::stretch::speed(samples, sample_rate, speed / NORMAL_SPEED);
            begin(&mut started);
            if !sink.play(&stretched, sample_rate, volume, &job.cancel, &shared.paused)? {
                return Ok(());
            }
            continue;
        }
        let result = render_piece(shared, voice, sink, job, (name, &piece), speed, volume, &mut started);
        match result {
            Err(e) if e == "cancelled" => return Ok(()),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
    }
    Ok(())
}

fn announce_start(shared: &Shared, req: &Request, started: &mut bool) {
    if !*started {
        *started = true;
        say(shared, req.client, format!("STARTED id={}", req.id));
    }
}

#[allow(clippy::too_many_arguments)]
fn render_piece(shared: &Shared, voice: &dyn Renderer, sink: &mut dyn Sink, job: &Job, (name, piece): (&str, &str), speed: f32, volume: u8, started: &mut bool) -> Result<(), String> {
    let req = &job.req;
    let begin = |started: &mut bool| announce_start(shared, req, started);

    let (tx, rx) = sync_channel::<Vec<i16>>(1);
    std::thread::scope(|scope| {
        let maker = scope.spawn(|| {
            let tx = tx;
            voice.render(name, piece, speed, &job.cancel, &mut |samples| tx.send(samples).is_ok())
        });
        let mut played = Ok(());
        for samples in rx.iter() {
            if job.cancel.load(Ordering::SeqCst) {
                break;
            }
            begin(started);
            match sink.play(&samples, VOICE_RATE, volume, &job.cancel, &shared.paused) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    played = Err(e);
                    break;
                }
            }
        }
        drop(rx);
        let made = maker.join().unwrap_or_else(|_| Err("the voice stopped".into()));
        played.and(made).map(|_| ())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    struct Fake {
        phrase: Option<Arc<Store>>,
        slow: Duration,
    }

    impl Renderer for Fake {
        fn render(&self, _: &str, text: &str, _: f32, cancel: &Arc<AtomicBool>, each: &mut dyn FnMut(Vec<i16>) -> bool) -> Result<u32, String> {
            for sentence in text.split_inclusive('.') {
                let t = Instant::now();
                while t.elapsed() < self.slow {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled".into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                if !each(vec![sentence.len() as i16; 2400]) {
                    break;
                }
            }
            Ok(24000)
        }
        fn status(&self) -> &'static str {
            "ready"
        }
        fn phrases(&self, _: &str) -> Option<Arc<Store>> {
            self.phrase.clone()
        }
    }

    struct Record(Sender<(usize, Instant)>, Duration);

    impl Sink for Record {
        fn play(&mut self, samples: &[i16], _: u32, _: u8, cancel: &AtomicBool, _: &AtomicBool) -> Result<bool, String> {
            let _ = self.0.send((samples.len(), Instant::now()));
            let t = Instant::now();
            while t.elapsed() < self.1 {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(false);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(true)
        }
    }

    fn engine(phrase: Option<Arc<Store>>, slow: u64, play: u64) -> (Engine, Receiver<String>, Receiver<(usize, Instant)>) {
        let (stx, srx) = channel();
        let e = Engine::start(Arc::new(Fake { phrase, slow: Duration::from_millis(slow) }), Box::new(Record(stx, Duration::from_millis(play))));
        let (tx, rx) = channel();
        e.connect(1, tx);
        (e, rx, srx)
    }

    fn req(id: &str, p: Priority, text: &str) -> Request {
        Request { client: 1, id: id.into(), priority: p, voice: "bf_emma".into(), rate: 50, volume: 80, text: text.into() }
    }

    fn lines(rx: &Receiver<String>, n: usize) -> Vec<String> {
        (0..n).map(|_| rx.recv_timeout(Duration::from_secs(3)).expect("an event")).collect()
    }

    #[test]
    fn a_sentence_is_started_then_done() {
        let (e, rx, _) = engine(None, 1, 5);
        e.submit(req("1", Priority::Normal, "One. Two."));
        assert_eq!(lines(&rx, 2), ["STARTED id=1", "DONE id=1"]);
    }

    #[test]
    fn a_stored_phrase_starts_without_rendering() {
        let store = Arc::new(Store::parse(&crate::speech::phrases::build(24000, &[("Save, button".into(), vec![5; 4800])])).unwrap());
        let (e, rx, srx) = engine(Some(store), 5000, 5);
        let t = Instant::now();
        e.submit(req("2", Priority::Focus, "Save, button"));
        assert_eq!(lines(&rx, 1), ["STARTED id=2"]);
        let (n, at) = srx.recv().unwrap();
        assert_eq!(n, 4800);
        assert!(at.duration_since(t) < Duration::from_millis(1000), "{:?}", at.duration_since(t));
    }

    #[test]
    fn a_focus_message_cancels_the_one_before_without_waiting_for_it_to_play() {
        let (e, rx, srx) = engine(None, 1, 2000);
        e.submit(req("a", Priority::Normal, "A long sentence."));
        assert_eq!(lines(&rx, 1), ["STARTED id=a"]);
        srx.recv().unwrap();
        let t = Instant::now();
        e.submit(req("b", Priority::Focus, "Next."));
        assert_eq!(lines(&rx, 1), ["CANCELLED id=a"]);
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        assert_eq!(lines(&rx, 1), ["STARTED id=b"]);
    }

    #[test]
    fn urgent_cuts_everything_and_focus_leaves_urgent_alone() {
        let (e, rx, srx) = engine(None, 1, 2000);
        e.submit(req("u", Priority::Urgent, "Important."));
        assert_eq!(lines(&rx, 1), ["STARTED id=u"]);
        srx.recv().unwrap();
        e.submit(req("n", Priority::Normal, "Queued."));
        e.submit(req("f", Priority::Focus, "Focus."));
        let got = lines(&rx, 1);
        assert_eq!(got, ["CANCELLED id=n"]);
        e.submit(req("x", Priority::Urgent, "Again."));
        let got = lines(&rx, 2);
        assert!(got.contains(&"CANCELLED id=u".to_string()) && got.contains(&"CANCELLED id=f".to_string()), "{got:?}");
    }

    #[test]
    fn normal_cuts_background_but_not_normal() {
        let (e, rx, srx) = engine(None, 1, 2000);
        e.submit(req("g", Priority::Background, "Quiet."));
        assert_eq!(lines(&rx, 1), ["STARTED id=g"]);
        srx.recv().unwrap();
        e.submit(req("n", Priority::Normal, "Now."));
        assert_eq!(lines(&rx, 1), ["CANCELLED id=g"]);
        assert_eq!(lines(&rx, 1), ["STARTED id=n"]);
        srx.recv().unwrap();
        e.submit(req("m", Priority::Normal, "Next."));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn stop_ends_only_that_clients_speech() {
        let (e, rx, srx) = engine(None, 1, 2000);
        e.submit(req("a", Priority::Normal, "One."));
        lines(&rx, 1);
        srx.recv().unwrap();
        e.submit(req("b", Priority::Normal, "Two."));
        e.stop(1);
        let got = lines(&rx, 2);
        assert!(got.contains(&"CANCELLED id=a".to_string()) && got.contains(&"CANCELLED id=b".to_string()), "{got:?}");
    }

    #[test]
    fn markers_are_reported_in_order() {
        let (e, rx, _) = engine(None, 1, 5);
        e.submit(req("m", Priority::Normal, "\u{1}1\u{1}First.\u{1}2\u{1}Second."));
        assert_eq!(lines(&rx, 4), ["MARKER id=m n=1", "STARTED id=m", "MARKER id=m n=2", "DONE id=m"].map(String::from));
    }

    #[test]
    fn text_is_cut_at_markers() {
        assert_eq!(parts("\u{1}1\u{1}A.\u{1}2\u{1}B."), vec![(Some(1), "A.".to_string()), (Some(2), "B.".to_string())]);
        assert_eq!(parts("Plain."), vec![(None, "Plain.".to_string())]);
    }

    #[test]
    fn rate_maps_to_speed() {
        assert!((speed_of(50) - NORMAL_SPEED).abs() < 1e-6);
        assert!(speed_of(0) < speed_of(100));
    }
}
