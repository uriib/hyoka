pub mod mpsc {
    use iced::futures::{SinkExt as _, StreamExt, channel::mpsc};

    pub struct UnboundedSender<T>(mpsc::UnboundedSender<T>);
    pub struct UnboundedReceiver<T>(mpsc::UnboundedReceiver<T>);
    impl<T> Clone for UnboundedSender<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }
    impl<T> UnboundedSender<T> {
        #[must_use]
        pub fn send(&self, msg: T) -> Option<()> {
            self.0.unbounded_send(msg).ok()
        }
    }
    impl<T> UnboundedReceiver<T> {
        pub async fn receive(&mut self) -> Option<T> {
            self.0.next().await
        }
    }
    pub fn unbounded<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
        let (tx, rx) = mpsc::unbounded();
        (UnboundedSender(tx), UnboundedReceiver(rx))
    }

    pub struct Sender<T>(mpsc::Sender<T>);
    pub struct Receiver<T>(mpsc::Receiver<T>);
    impl<T> Clone for Sender<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }
    impl<T> Sender<T> {
        #[must_use]
        pub async fn send(&mut self, msg: T) -> Option<()> {
            self.0.send(msg).await.ok()
        }
        #[allow(dead_code)]
        #[must_use]
        pub async fn feed(&mut self, msg: T) -> Option<()> {
            self.0.feed(msg).await.ok()
        }
        #[allow(dead_code)]
        #[must_use]
        pub async fn flush(&mut self) -> Option<()> {
            self.0.flush().await.ok()
        }
        #[allow(dead_code)]
        #[must_use]
        pub fn try_send(&mut self, msg: T) -> Option<()> {
            self.0.try_send(msg).ok()
        }
    }
    impl<T> Receiver<T> {
        pub async fn receive(&mut self) -> Option<T> {
            self.0.next().await
        }
        /// None can be no masage or error
        #[allow(dead_code)]
        pub fn try_receive(&mut self) -> Option<T> {
            match self.0.try_next() {
                Ok(Some(x)) => Some(x),
                _ => None,
            }
        }
    }
    pub fn channel<T>(buffer: usize) -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = mpsc::channel(buffer);
        (Sender(tx), Receiver(rx))
    }
}
