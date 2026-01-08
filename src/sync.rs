pub mod mpsc {
    use iced::futures::{SinkExt as _, StreamExt, channel::mpsc};

    pub struct UnboundedSender<T>(mpsc::UnboundedSender<T>);
    pub struct UnboundedReceiver<T>(mpsc::UnboundedReceiver<T>);
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
    impl<T> Sender<T> {
        #[must_use]
        pub async fn send(&mut self, msg: T) -> Option<()> {
            self.0.send(msg).await.ok()
        }
    }
    impl<T> Receiver<T> {
        pub async fn receive(&mut self) -> Option<T> {
            self.0.next().await
        }
    }
    pub fn channel<T>(buffer: usize) -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = mpsc::channel(buffer);
        (Sender(tx), Receiver(rx))
    }
}
