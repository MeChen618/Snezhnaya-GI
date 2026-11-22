use crate::handler::PacketHandler;
use common::time_util;
use kcp::Kcp;
use std::{net::SocketAddr, sync::Arc, task};
use tokio::{io::AsyncWrite, net::UdpSocket, sync::mpsc};

struct UdpOutput {
    peer_addr: SocketAddr,
    socket: Arc<UdpSocket>,
}

pub enum NetEvent {
    Recv(Box<[u8]>),
    Send(Box<[u8]>),
    Close(),
}

pub fn start(
    conv: u32,
    token: u32,
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
    handler: PacketHandler,
) -> mpsc::Sender<NetEvent> {
    let (tx, rx) = mpsc::channel(32);
    let kcp = Kcp::new(
        conv,
        token,
        time_util::unix_timestamp_ms(),
        false,
        UdpOutput { peer_addr, socket },
    );

    tokio::spawn(async move { kcp_loop(kcp, rx, handler).await });
    tx
}

async fn kcp_loop(
    mut kcp: Kcp<UdpOutput>,
    mut rx: mpsc::Receiver<NetEvent>,
    handler: PacketHandler,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(10));
    let mut recv_buf = Vec::new();
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            event = rx.recv() => match event {
                Some(NetEvent::Recv(buf)) => {
                    if let Err(error) = kcp.input(&buf) {
                        tracing::debug!(%error, "discarding malformed KCP datagram");
                        continue;
                    }
                }
                Some(NetEvent::Send(buf)) => {
                    if let Err(error) = kcp.send(&buf) {
                        tracing::warn!(%error, "KCP send rejected");
                    }
                }
                Some(NetEvent::Close()) | None => {
                    handler.remove_connection(kcp.conv());
                    break;
                }
            }
        }
        // 即使没有新包，也要推进 ACK 和重传计时。
        if let Err(error) = kcp
            .async_update((time_util::unix_timestamp_ms() - kcp.start_ts()) as u32)
            .await
        {
            tracing::debug!(%error, "KCP connection closed");
            handler.remove_connection(kcp.conv());
            break;
        }
        while let Ok(size) = kcp.peeksize() {
            if size > 4 * 1024 * 1024 {
                tracing::warn!(size, "KCP message exceeds receive limit");
                handler.remove_connection(kcp.conv());
                return;
            }
            recv_buf.resize(size, 0);
            match kcp.recv(&mut recv_buf) {
                Ok(length) => handler.enqueue(kcp.conv(), recv_buf[..length].into()),
                Err(_) => break,
            }
        }
    }
}

impl AsyncWrite for UdpOutput {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> task::Poll<Result<usize, std::io::Error>> {
        self.socket.poll_send_to(cx, buf, self.peer_addr)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut task::Context<'_>,
    ) -> task::Poll<Result<(), std::io::Error>> {
        task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut task::Context<'_>,
    ) -> task::Poll<Result<(), std::io::Error>> {
        task::Poll::Ready(Ok(()))
    }
}
