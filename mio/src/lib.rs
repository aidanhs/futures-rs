extern crate mio;
extern crate futures;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::panic;
use std::slice;
use std::sync::Arc;
use std::sync::mpsc::{channel, TryRecvError};

use futures::{Future, promise, Complete, PollError};
// use slot::Slot;
//
// thread_local!{
//     pub static INNER: Arc<Inner> = Arc::new(Inner {
//         poll: mio::Poll::new().unwrap(),
//     })
// }

pub type IoFuture<T> = Future<Item=T, Error=io::Error>;

pub struct Loop {
    io: mio::Poll,
    tx: mio::channel::Sender<Message>,
    rx: mio::channel::Receiver<Message>,
    next: usize,
    done: HashMap<usize, Complete<(), io::Error>>,
}

enum Message {
    Wait(Complete<(), io::Error>, mio::EventSet, Arc<mio::Evented + Send + Sync>),
    Register(Arc<mio::Evented + Send + Sync>),
}

// pub struct TcpConnect {
//     tcp: mio::tcp::TcpStream,
//     slot: Arc<Slot<mio::EventSet>>,
//     inner: Arc<Inner>,
// }
//
// impl Future for TcpConnect {
//     type Item = TcpStream;
//     type Error = io::Error;
//
//     fn poll(self) -> Result<io::Result<TcpStream>, TcpConnect> {
//         match self.slot.try_consume() {
//             Ok(_events) => Ok(Ok(TcpStream::new(self.tcp, self.inner))),
//             Err(..) => Err(self),
//         }
//     }
//
//     fn schedule<G>(self, g: G)
//         where G: FnOnce(Result<Self::Item, Self::Error>) + Send + 'static
//     {
//         self.slot.clone().on_full(move |_events| {
//             g(Ok(TcpStream::new(self.tcp, self.inner)))
//         });
//     }
// }
//

pub struct TcpListener {
    tcp: Arc<mio::tcp::TcpListener>,
    tx: mio::channel::Sender<Message>,
}

impl TcpListener {
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }

    pub fn accept(&self) -> Box<IoFuture<(TcpStream, SocketAddr)>> {
        match self.tcp.accept() {
            Err(e) => return futures::failed(e).boxed(),
            Ok(Some((tcp, addr))) => {
                let tcp = TcpStream {
                    tcp: Arc::new(tcp),
                    tx: self.tx.clone(),
                };
                let res = self.tx.send(Message::Register(tcp.tcp.clone()));
                let res = res.map(|()| (tcp, addr));
                let res = res.map_err(|e| {
                    match e {
                        mio::channel::SendError::Io(e) => e,
                        // TODO: need to handle a closed channel
                        mio::channel::SendError::Disconnected(..) => {
                            panic!("closed channel")
                        }
                    }
                });
                return futures::done(res).boxed()
            }
            Ok(None) => {}
        }

        let (p, c) = promise();
        let r = self.tx.send(Message::Wait(c,
                                           mio::EventSet::readable(),
                                           self.tcp.clone()));
        match r {
            Ok(()) => {
                let me = TcpListener {
                    tcp: self.tcp.clone(),
                    tx: self.tx.clone(),
                };
                p.and_then(move |()| me.accept()).boxed()
            }
            Err(mio::channel::SendError::Io(e)) => {
                return futures::failed(e).boxed()
            }
            Err(mio::channel::SendError::Disconnected(..)) => panic!("closed channel"),
        }
    }
}

// impl Future for TcpListener {
//     type Item = (TcpStream, SocketAddr, TcpListener);
//     type Error = io::Error;
//
//     fn poll(self) -> Result<io::Result<(TcpStream, SocketAddr, TcpListener)>,
//                                        TcpListener> {
//         match self.tcp.accept() {
//             Ok(Some((stream, addr))) => {
//                 let stream = TcpStream::new(stream, self.inner.clone());
//                 Ok(Ok((stream, addr, self)))
//             }
//             Ok(None) => Err(self),
//             Err(e) => Ok(Err(e)),
//         }
//     }
//
//     fn schedule<G>(self, g: G)
//         where G: FnOnce(Result<Self::Item, Self::Error>) + Send + 'static
//     {
//         let me = match self.poll() {
//             Ok(item) => return g(item),
//             Err(me) => me,
//         };
//         let res = me.inner.poll.register(&me.tcp,
//                                          slot2token(me.slot.clone()),
//                                          mio::EventSet::readable(),
//                                          mio::PollOpt::edge() |
//                                              mio::PollOpt::oneshot());
//         if let Err(e) = res {
//             return g(Err(e))
//         }
//         me.slot.clone().on_full(move |slot| {
//             slot.try_consume().ok().unwrap();
//             me.schedule(g)
//         });
//     }
// }

pub struct TcpStream {
    tcp: Arc<mio::tcp::TcpStream>,
    tx: mio::channel::Sender<Message>,
}

unsafe fn slice_to_end(v: &mut Vec<u8>) -> &mut [u8] {
    slice::from_raw_parts_mut(v.as_mut_ptr().offset(v.len() as isize),
                              v.capacity() - v.len())
}

impl TcpStream {
//     fn new(tcp: mio::tcp::TcpStream, inner: Arc<Inner>) -> TcpStream {
//         TcpStream {
//             tcp: tcp,
//             inner: inner,
//         }
//     }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.peer_addr()
    }

    pub fn read(&self, mut into: Vec<u8>) -> Box<IoFuture<Vec<u8>>> {
        let r = unsafe {
            (&*self.tcp).read(slice_to_end(&mut into))
        };
        match r {
            Ok(i) => {
                unsafe {
                    let len = into.len();
                    into.set_len(len + i);
                }
                return futures::finished(into).boxed()
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    return futures::failed(e).boxed()
                }
            }
        }
        let (p, c) = promise();
        let r = self.tx.send(Message::Wait(c,
                                           mio::EventSet::readable(),
                                           self.tcp.clone()));
        match r {
            Ok(()) => {
                let me2 = TcpStream {
                    tcp: self.tcp.clone(),
                    tx: self.tx.clone(),
                };
                p.and_then(move |()| me2.read(into)).boxed()
            }
            Err(mio::channel::SendError::Io(e)) => {
                return futures::failed(e).boxed()
            }
            Err(mio::channel::SendError::Disconnected(..)) => panic!("closed channel"),
        }
    }

    pub fn write(&self, offset: usize, data: Vec<u8>)
                 -> Box<IoFuture<(usize, Vec<u8>)>> {
        let r = (&*self.tcp).write(&data[offset..]);
        match r {
            Ok(i) => return futures::finished((offset + i, data)).boxed(),
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    return futures::failed(e).boxed()
                }
            }
        }
        let (p, c) = promise();
        let r = self.tx.send(Message::Wait(c,
                                           mio::EventSet::writable(),
                                           self.tcp.clone()));
        match r {
            Ok(()) => {
                let me2 = TcpStream {
                    tcp: self.tcp.clone(),
                    tx: self.tx.clone(),
                };
                p.and_then(move |()| me2.write(offset, data)).boxed()
            }
            Err(mio::channel::SendError::Io(e)) => {
                return futures::failed(e).boxed()
            }
            Err(mio::channel::SendError::Disconnected(..)) => panic!("closed channel"),
        }
    }

//     pub fn write(self, buf: Vec<u8>) -> io::Result<TcpWrite> {
//         let slot = Arc::new(Slot::new(None));
//         Ok(TcpWrite {
//             stream: self,
//             buf: buf,
//             slot: slot,
//         })
//     }
//
//     fn try_read(&self, into: &mut Vec<u8>) -> io::Result<Option<usize>> {
//         let mut tcp = &self.tcp;
//         unsafe {
//             let cur = into.len();
//             let dst = into.as_mut_ptr().offset(cur as isize);
//             let len = into.capacity() - cur;
//             match tcp.try_read(slice::from_raw_parts_mut(dst, len)) {
//                 Ok(Some(amt)) => {
//                     into.set_len(cur + amt);
//                     Ok(Some(amt))
//                 }
//                 other => other,
//             }
//         }
//     }
}
//
// pub struct TcpRead {
//     stream: TcpStream,
//     buf: Vec<u8>,
//     slot: Arc<Slot<mio::EventSet>>,
// }
//
// impl Future for TcpRead {
//     type Item = (Vec<u8>, usize, TcpStream);
//     type Error = io::Error;
//
//     fn poll(mut self) -> Result<io::Result<Self::Item>, Self> {
//         match self.stream.try_read(&mut self.buf) {
//             Ok(Some(amt)) => Ok(Ok((self.buf, amt, self.stream))),
//             Ok(None) => Err(self),
//             Err(e) => Ok(Err(e)),
//         }
//     }
//
//     fn schedule<G>(self, g: G)
//         where G: FnOnce(Result<Self::Item, Self::Error>) + Send + 'static
//     {
//         let me = match self.poll() {
//             Ok(item) => return g(item),
//             Err(me) => me,
//         };
//         let res = me.stream.inner.poll.register(&me.stream.tcp,
//                                                 slot2token(me.slot.clone()),
//                                                 mio::EventSet::readable(),
//                                                 mio::PollOpt::edge() |
//                                                         mio::PollOpt::oneshot());
//         if let Err(e) = res {
//             return g(Err(e))
//         }
//         me.slot.clone().on_full(move |slot| {
//             slot.try_consume().ok().unwrap();
//             me.schedule(g)
//         });
//     }
// }
//
// pub struct TcpWrite {
//     stream: TcpStream,
//     buf: Vec<u8>,
//     slot: Arc<Slot<mio::EventSet>>,
// }
//
// impl Future for TcpWrite {
//     type Item = (Vec<u8>, usize, TcpStream);
//     type Error = io::Error;
//
//     fn poll(mut self) -> Result<io::Result<Self::Item>, Self> {
//         match (&self.stream.tcp).try_write(&mut self.buf) {
//             Ok(Some(amt)) => Ok(Ok((self.buf, amt, self.stream))),
//             Ok(None) => Err(self),
//             Err(e) => Ok(Err(e)),
//         }
//     }
//
//     fn schedule<G>(self, g: G)
//         where G: FnOnce(Result<Self::Item, Self::Error>) + Send + 'static
//     {
//         let me = match self.poll() {
//             Ok(item) => return g(item),
//             Err(me) => me,
//         };
//         let res = me.stream.inner.poll.register(&me.stream.tcp,
//                                                 slot2token(me.slot.clone()),
//                                                 mio::EventSet::writable(),
//                                                 mio::PollOpt::edge() |
//                                                         mio::PollOpt::oneshot());
//         if let Err(e) = res {
//             return g(Err(e))
//         }
//         me.slot.clone().on_full(move |slot| {
//             slot.try_consume().ok().unwrap();
//             me.schedule(g)
//         });
//     }
// }

impl Loop {
    pub fn new() -> io::Result<Loop> {
        let (tx, rx) = mio::channel::from_std_channel(channel());
        let io = try!(mio::Poll::new());
        try!(io.register(&rx,
                         mio::Token(0),
                         mio::EventSet::readable(),
                         mio::PollOpt::edge()));
        Ok(Loop {
            io: io,
            done: HashMap::new(),
            next: 1,
            tx: tx,
            rx: rx,
        })
    }

    pub fn await<F: Future>(&mut self, mut f: F)
                            -> Result<F::Item, F::Error> {
        let (tx, rx) = channel();
        f.schedule(move |r| {
            drop(tx.send(r))
            // TODO: signal to the event loop that it should wake up
        });
        let mut ret = None;
        self._await(&mut || {
            match rx.try_recv() {
                Ok(e) => ret = Some(e),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => panic!(),
            }
            ret.is_some()
        });
        match ret.unwrap() {
            Ok(e) => Ok(e),
            Err(PollError::Other(e)) => Err(e),
            Err(PollError::Panicked(p)) => panic::resume_unwind(p),
            Err(PollError::Canceled) => panic!("canceled"),
        }
    }

    fn _await(&mut self, done: &mut FnMut() -> bool) {
        while !done() {
            let amt = self.io.poll(None).unwrap();

            for i in 0..amt {
                let event = self.io.events().get(i).unwrap();
                let token = event.token().as_usize();
                if token == 0 {
                    while let Ok(msg) = self.rx.try_recv() {
                        self.notify(msg);
                    }
                } else if let Some(complete) = self.done.remove(&token) {
                    complete.finish(());
                }
            }
        }
    }

    fn notify(&mut self, msg: Message) {
        match msg {
            Message::Wait(c, events, evented) => {
                let token = self.next;
                self.next += 1;
                let evented: &mio::Evented = &*evented;
                let r = self.io.reregister(evented,
                                           mio::Token(token),
                                           events,
                                           mio::PollOpt::edge() |
                                              mio::PollOpt::oneshot());
                match r {
                    Ok(()) => {
                        self.done.insert(token, c);
                    }
                    Err(e) => c.fail(e),
                }
            }
            Message::Register(evented) => {
                // TODO: propagate this error somewhere
                let evented: &mio::Evented = &*evented;
                self.io.register(evented,
                                 mio::Token(0),
                                 mio::EventSet::none(),
                                 mio::PollOpt::empty()).unwrap();
            }
        }
    }

    pub fn tcp_connect(&mut self, addr: &SocketAddr)
                       -> Box<IoFuture<TcpStream>> {
        let pair = mio::tcp::TcpStream::connect(addr).and_then(|tcp| {
            let token = self.next;
            self.next += 1;
            try!(self.io.register(&tcp,
                                  mio::Token(token),
                                  mio::EventSet::writable(),
                                  mio::PollOpt::edge() |
                                    mio::PollOpt::oneshot()));
            Ok((tcp, token))
        });
        match pair {
            Ok((tcp, token)) => {
                let (p, c) = promise();
                assert!(self.done.insert(token, c).is_none());
                let tx = self.tx.clone();
                p.map(|()| {
                    TcpStream {
                        tcp: Arc::new(tcp),
                        tx: tx,
                    }
                }).boxed()
            }
            Err(e) => futures::failed(e).boxed(),
        }
    }

    pub fn tcp_listen(&mut self, addr: &SocketAddr) -> io::Result<TcpListener> {
        let tcp = try!(mio::tcp::TcpListener::bind(addr));
        try!(self.io.register(&tcp,
                              mio::Token(0),
                              mio::EventSet::none(),
                              mio::PollOpt::empty()));

        Ok(TcpListener {
            tcp: Arc::new(tcp),
            tx: self.tx.clone(),
        })
    }
}
//
// impl Inner {
//     pub fn await(&self, slot: &Slot<()>) {
//         let (reader, mut writer) = mio::unix::pipe().unwrap();
//         let mut events = mio::Events::new();
//         let mut done = false;
//         slot.on_full(move |_slot| {
//             use std::io::Write;
//             writer.write(&[1]).unwrap();
//         });
//         self.poll.register(&reader, mio::Token(0),
//                            mio::EventSet::readable(),
//                            mio::PollOpt::edge()).unwrap();
//         while !done {
//             self.poll.poll(&mut events, None).unwrap();
//             for event in events.iter() {
//                 if event.token() == mio::Token(0) {
//                     done = true;
//                     continue
//                 }
//                 let slot = unsafe { token2slot(event.token()) };
//                 let kind = event.kind();
//                 slot.on_empty(move |complete| {
//                     complete.try_produce(kind).ok().unwrap();
//                 });
//             }
//         }
//     }
// }
//
// unsafe fn token2slot(token: mio::Token) -> Arc<Slot<mio::EventSet>> {
//     mem::transmute(token.as_usize())
// }
//
// fn slot2token(c: Arc<Slot<mio::EventSet>>) -> mio::Token {
//     unsafe { mio::Token(mem::transmute::<_, usize>(c)) }
// }

// impl mio::Handler for Inner {
//     type Timeout = ();
//     type Message = Message;
//
// }