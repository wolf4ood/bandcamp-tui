//! Terminal events and the redraw tick, forwarded as `Message`s.

use std::time::Duration;

use futures::StreamExt;
use ratatui::crossterm::event::EventStream;
use tokio::sync::mpsc::UnboundedSender;

use super::Message;

const TICK: Duration = Duration::from_millis(250);

pub fn spawn(tx: UnboundedSender<Message>) {
    tokio::spawn(async move {
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if tx.send(Message::Tick).is_err() {
                        break;
                    }
                }
                event = events.next() => match event {
                    Some(Ok(event)) => {
                        if tx.send(Message::Terminal(event)).is_err() {
                            break;
                        }
                    }
                    Some(Err(err)) => {
                        tracing::error!(%err, "terminal event stream failed");
                        break;
                    }
                    None => break,
                },
            }
        }
    });
}
