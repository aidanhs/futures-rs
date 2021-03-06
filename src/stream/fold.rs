use std::mem;

use {Task, Future, Poll, IntoFuture};
use stream::Stream;
use util::Collapsed;

/// A future used to collect all the results of a stream into one generic type.
///
/// This future is returned by the `Stream::fold` method.
pub struct Fold<S, F, Fut, T> where Fut: IntoFuture {
    stream: S,
    f: F,
    state: State<T, Fut::Future>,
}

enum State<T, F> where F: Future {
    /// Placeholder state when doing work
    Empty,

    /// Ready to process the next stream item; current accumulator is the `T`
    Ready(T),

    /// Working on a future the process the previous stream item
    Processing(Collapsed<F>),
}

pub fn new<S, F, Fut, T>(s: S, f: F, t: T) -> Fold<S, F, Fut, T>
    where S: Stream,
          F: FnMut(T, S::Item) -> Fut + 'static,
          Fut: IntoFuture<Item = T>,
          Fut::Error: Into<S::Error>,
          T: 'static
{
    Fold {
        stream: s,
        f: f,
        state: State::Ready(t),
    }
}

impl<S, F, Fut, T> Future for Fold<S, F, Fut, T>
    where S: Stream,
          F: FnMut(T, S::Item) -> Fut + 'static,
          Fut: IntoFuture<Item = T>,
          Fut::Error: Into<S::Error>,
          T: 'static
{
    type Item = T;
    type Error = S::Error;

    fn poll(&mut self, task: &mut Task) -> Poll<T, S::Error> {
        loop {
            match mem::replace(&mut self.state, State::Empty) {
                State::Empty => panic!("cannot poll Fold twice"),
                State::Ready(state) => {
                    match self.stream.poll(task) {
                        Poll::Ok(Some(e)) => {
                            let future = (self.f)(state, e);
                            let future = future.into_future();
                            let future = Collapsed::Start(future);
                            self.state = State::Processing(future);
                        }
                        Poll::Ok(None) => return Poll::Ok(state),
                        Poll::Err(e) => return Poll::Err(e),
                        Poll::NotReady => {
                            self.state = State::Ready(state);
                            return Poll::NotReady
                        }
                    }
                }
                State::Processing(mut fut) => {
                    match fut.poll(task) {
                        Poll::Ok(state) => self.state = State::Ready(state),
                        Poll::Err(e) => return Poll::Err(e.into()),
                        Poll::NotReady => {
                            self.state = State::Processing(fut);
                            return Poll::NotReady;
                        }
                    }
                }
            }
        }
    }

    fn schedule(&mut self, task: &mut Task) {
        match self.state {
            State::Empty => panic!("cannot `schedule` a completed Fold"),
            State::Ready(_) => self.stream.schedule(task),
            State::Processing(ref mut fut) => fut.schedule(task),
        }
    }

    unsafe fn tailcall(&mut self)
                       -> Option<Box<Future<Item=Self::Item, Error=Self::Error>>> {
        if let State::Processing(ref mut fut) = self.state {
            fut.collapse();
        }
        None
    }
}
