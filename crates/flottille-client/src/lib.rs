//! A client of the daemon on one connection (`docs/protocol.md`, "Transport" and "Framing"):
//! requests and their answers, with or without a bound, and the events of a subscription, which a
//! watch resumes after the last one seen. Blocking, on any stream whose reads can be bounded in
//! time: the Unix socket today, the link's later.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, ErrorKind};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use flottille_protocol::{ErrorBody, Event, Message, Request, WatchParams};
use serde_json::Value;

/// A stream the daemon is read through.
pub trait Stream: io::Read + io::Write {
    /// Bounds the reads that follow; `None`: they take as long as they take.
    fn set_read_timeout(&self, wait: Option<Duration>) -> io::Result<()>;
}

impl Stream for UnixStream {
    fn set_read_timeout(&self, wait: Option<Duration>) -> io::Result<()> {
        UnixStream::set_read_timeout(self, wait)
    }
}

/// What went wrong with the connection itself; an error the daemon answered is an [`Answer`].
#[derive(Debug)]
pub enum Error {
    /// The stream failed.
    Io(io::Error),
    /// The daemon closed the connection.
    Closed,
    /// The daemon did not answer within the bound. Its late answer would come to the next
    /// request: the connection is not to be used again.
    TimedOut,
    /// The daemon wrote a line that is not a message.
    NotAMessage(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(error) => write!(f, "{error}"),
            Error::Closed => f.write_str("the daemon closed the connection"),
            Error::TimedOut => f.write_str("the daemon did not answer in time"),
            Error::NotAMessage(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            Error::NotAMessage(error) => Some(error),
            Error::Closed | Error::TimedOut => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

/// The daemon's answer to a request: its result, or the error it answered with.
pub type Answer = std::result::Result<Value, ErrorBody>;

/// One connection: requests answered in turn, the events that come meanwhile kept for
/// [`Connection::next_event`].
pub struct Connection<S: Stream> {
    reader: BufReader<S>,
    next_id: u64,
    /// The start of a line a timeout cut short, kept as bytes: a cut can fall inside a character.
    partial: Vec<u8>,
    /// Events that came while a request waited for its answer, the oldest first.
    queued: VecDeque<Event>,
}

impl<S: Stream> Connection<S> {
    pub fn new(stream: S) -> Connection<S> {
        Connection {
            reader: BufReader::new(stream),
            next_id: 1,
            partial: Vec::new(),
            queued: VecDeque::new(),
        }
    }

    /// The stream, for a client that speaks the protocol on its own. Taken before anything was
    /// read: what is read ahead is lost with the connection.
    pub fn into_stream(self) -> S {
        self.reader.into_inner()
    }

    /// Sends a request without waiting for its answer: its id, for [`Connection::answer`].
    pub fn send(&mut self, method: &str, params: Value) -> Result<String, Error> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let request = Request {
            id: id.clone(),
            method: method.to_owned(),
            params,
        };
        let mut line = serde_json::to_vec(&request).map_err(io::Error::other)?;
        line.push(b'\n');
        let stream = self.reader.get_mut();
        stream.write_all(&line)?;
        stream.flush()?;
        Ok(id)
    }

    /// Sends a request and waits for its answer as long as it takes.
    pub fn ask(&mut self, method: &str, params: Value) -> Result<Answer, Error> {
        let id = self.send(method, params)?;
        self.answer(&id, None)
    }

    /// [`Connection::ask`], waiting at most `bound`: past it, [`Error::TimedOut`].
    pub fn ask_within(
        &mut self,
        method: &str,
        params: Value,
        bound: Duration,
    ) -> Result<Answer, Error> {
        let deadline = Instant::now() + bound;
        let id = self.send(method, params)?;
        self.answer(&id, Some(deadline))
    }

    /// Subscribes to the events (`events.watch`): with `after`, the stored ones after that number
    /// come first, which resumes a watch after the last event seen.
    pub fn watch(&mut self, params: WatchParams) -> Result<Answer, Error> {
        let params = serde_json::to_value(params).map_err(io::Error::other)?;
        self.ask("events.watch", params)
    }

    /// The answer to the request `id`, waiting until `deadline` at most (`None`: as long as it
    /// takes). The answers to other requests are skipped; the events are kept.
    pub fn answer(&mut self, id: &str, deadline: Option<Instant>) -> Result<Answer, Error> {
        loop {
            let wait = match deadline {
                None => None,
                Some(at) => {
                    let left = at.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(Error::TimedOut);
                    }
                    Some(left)
                }
            };
            let message = match self.read(wait) {
                Err(Error::TimedOut) => continue,
                read => read?,
            };
            if let Some(event) = message.event {
                self.queued.push_back(event);
                continue;
            }
            if message.id.as_deref() != Some(id) {
                continue;
            }
            return Ok(match message.error {
                Some(error) => Err(error),
                None => Ok(message.result.unwrap_or(Value::Null)),
            });
        }
    }

    /// The next event, a kept one first, waiting at most `wait` (`None`: as long as it takes);
    /// `None` when none came in time. Answers are skipped.
    pub fn next_event(&mut self, wait: Option<Duration>) -> Result<Option<Event>, Error> {
        if let Some(event) = self.queued.pop_front() {
            return Ok(Some(event));
        }
        let deadline = wait.map(|wait| Instant::now() + wait);
        loop {
            let left = deadline.map(|at| at.saturating_duration_since(Instant::now()));
            if left == Some(Duration::ZERO) {
                return Ok(None);
            }
            match self.read(left) {
                Ok(Message {
                    event: Some(event), ..
                }) => return Ok(Some(event)),
                Ok(_) => {}
                Err(Error::TimedOut) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }

    /// The oldest event kept while a request waited for its answer, without reading.
    pub fn queued(&mut self) -> Option<Event> {
        self.queued.pop_front()
    }

    /// The next message, waiting at most `wait`.
    fn read(&mut self, wait: Option<Duration>) -> Result<Message, Error> {
        self.reader.get_ref().set_read_timeout(wait)?;
        let read = self.reader.read_until(b'\n', &mut self.partial);
        let _ = self.reader.get_ref().set_read_timeout(None);
        match read {
            // The end of the stream, at a line's end or within one.
            Ok(0) => Err(Error::Closed),
            Ok(_) if !self.partial.ends_with(b"\n") => Err(Error::Closed),
            Ok(_) => {
                let line = std::mem::take(&mut self.partial);
                serde_json::from_slice(&line).map_err(Error::NotAMessage)
            }
            // What was read of the line before the timeout stays in `partial` for the next read.
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Err(Error::TimedOut)
            }
            Err(error) => Err(Error::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use serde_json::json;

    /// The daemon's end of a connection, scripted by the test.
    struct Daemon {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
    }

    impl Daemon {
        fn request(&mut self) -> Request {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }

        fn write(&mut self, message: Value) {
            self.writer
                .write_all(format!("{message}\n").as_bytes())
                .unwrap();
        }
    }

    fn connected() -> (Connection<UnixStream>, Daemon) {
        let (client, daemon) = UnixStream::pair().unwrap();
        let daemon = Daemon {
            reader: BufReader::new(daemon.try_clone().unwrap()),
            writer: daemon,
        };
        (Connection::new(client), daemon)
    }

    fn event(seq: u64) -> Value {
        json!({"event": {"seq": seq, "at": 0, "name": "x.changed", "data": {}}})
    }

    #[test]
    fn a_request_gets_its_answer_and_not_another_one() {
        let (mut client, mut daemon) = connected();
        let answering = std::thread::spawn(move || {
            let asked = daemon.request();
            assert_eq!(asked.method, "project.list");
            assert_eq!(asked.params, json!({"all": true}));
            daemon.write(json!({"id": "another", "result": {"no": 1}}));
            daemon.write(json!({"id": asked.id, "result": {"projects": []}}));
            let asked = daemon.request();
            daemon.write(json!({"id": asked.id, "error": {"code": "not-found", "message": "no"}}));
            daemon
        });

        let answer = client.ask("project.list", json!({"all": true})).unwrap();
        assert_eq!(answer, Ok(json!({"projects": []})));
        let refused = client.ask("project.show", json!({"name": "x"})).unwrap();
        assert_eq!(
            refused,
            Err(ErrorBody {
                code: "not-found".into(),
                message: "no".into()
            })
        );
        answering.join().unwrap();
    }

    #[test]
    fn an_event_between_answers_is_kept_for_the_next_event() {
        let (mut client, mut daemon) = connected();
        let answering = std::thread::spawn(move || {
            let asked = daemon.request();
            daemon.write(event(4));
            daemon.write(event(5));
            daemon.write(json!({"id": asked.id, "result": null}));
            daemon.write(event(6));
            daemon
        });

        assert_eq!(
            client.ask("daemon.status", json!({})).unwrap(),
            Ok(Value::Null)
        );
        let _daemon = answering.join().unwrap();
        assert_eq!(client.queued().map(|event| event.seq), Some(4));
        let next = client.next_event(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(next.map(|event| event.seq), Some(5));
        let next = client.next_event(None).unwrap();
        assert_eq!(next.map(|event| event.seq), Some(6));
        assert!(client.queued().is_none());
    }

    #[test]
    fn a_request_not_answered_within_its_bound_times_out() {
        let (mut client, mut daemon) = connected();

        let began = Instant::now();
        let asked = client.ask_within("daemon.status", json!({}), Duration::from_millis(100));
        assert!(matches!(asked, Err(Error::TimedOut)), "{asked:?}");
        assert!(began.elapsed() >= Duration::from_millis(100));
        assert_eq!(daemon.request().method, "daemon.status");
        // No event within the wait: none, and the connection goes on.
        let next = client.next_event(Some(Duration::from_millis(50))).unwrap();
        assert!(next.is_none());
        daemon.write(event(1));
        let next = client.next_event(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(next.map(|event| event.seq), Some(1));
    }

    #[test]
    fn a_watch_resumes_after_the_last_event_seen() {
        let (mut client, mut daemon) = connected();
        let answering = std::thread::spawn(move || {
            let asked = daemon.request();
            assert_eq!(asked.method, "events.watch");
            assert_eq!(asked.params, json!({"after": 7, "client": "window"}));
            daemon.write(json!({"id": asked.id, "result": {"after": 9}}));
            daemon.write(event(8));
            daemon.write(event(9));
            daemon
        });

        let started = client
            .watch(WatchParams {
                after: Some(7),
                client: Some("window".into()),
            })
            .unwrap();
        assert_eq!(started, Ok(json!({"after": 9})));
        let _daemon = answering.join().unwrap();
        for seq in [8, 9] {
            let next = client.next_event(Some(Duration::from_secs(5))).unwrap();
            assert_eq!(next.map(|event| event.seq), Some(seq));
        }
    }

    #[test]
    fn a_line_cut_by_a_timeout_is_read_whole_once_it_ends() {
        let (mut client, mut daemon) = connected();
        let line = format!(
            "{}\n",
            json!({"event": {"seq": 3, "at": 0, "name": "x.é", "data": {}}})
        );
        // Cut inside the two bytes of the `é`.
        let cut = line.find('é').unwrap() + 1;

        daemon.writer.write_all(&line.as_bytes()[..cut]).unwrap();
        let next = client.next_event(Some(Duration::from_millis(50))).unwrap();
        assert!(next.is_none());
        daemon.writer.write_all(&line.as_bytes()[cut..]).unwrap();
        let next = client
            .next_event(Some(Duration::from_secs(5)))
            .unwrap()
            .unwrap();
        assert_eq!((next.seq, next.name.as_str()), (3, "x.é"));
    }

    #[test]
    fn a_connection_the_daemon_closed_says_it() {
        let (mut client, daemon) = connected();
        drop(daemon);

        assert!(matches!(client.next_event(None), Err(Error::Closed)));
        assert!(matches!(
            client.next_event(Some(Duration::from_secs(5))),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn a_line_that_is_not_a_message_is_an_error() {
        let (mut client, mut daemon) = connected();
        daemon.writer.write_all(b"not json\n").unwrap();

        let next = client.next_event(Some(Duration::from_secs(5)));
        assert!(matches!(next, Err(Error::NotAMessage(_))), "{next:?}");
    }
}
