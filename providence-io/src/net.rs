use std::{
    io::{self},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
    ops::ControlFlow,
    sync::Arc,
    time::Duration,
};

use pawawwewism::reactive::{Disconnected, Reader, Value};
use uwuhi_async::{
    name::Label,
    resolver::{AsyncResolver, SyncResolver},
    service::{
        advertising::AsyncAdvertiser,
        discovery::{AsyncDiscoverer, SyncDiscoverer},
        InstanceDetails, Service, ServiceInstance, ServiceTransport,
    },
};

use crate::{data::TrackingMessage, drop::defer, task::Task};

const SERVICE: &str = "_providence";

pub struct Publisher {
    port: u16,
    message: Value<Option<Arc<TrackingMessage>>>,
    connections_reader: Reader<usize>,
    _advertiser: Task<io::Result<()>>,
    _listener: Task<io::Result<()>>,
}

impl Publisher {
    pub fn spawn() -> io::Result<Self> {
        let local_addrs = if_addrs::get_if_addrs()?
            .into_iter()
            .filter_map(|interface| match interface.ip() {
                IpAddr::V4(ip) if ip.is_private() => Some(ip),
                _ => None,
            })
            .collect::<Vec<_>>();

        log::info!("local private network addresses: {:?}", local_addrs);
        let (&first_addr, more_addrs) = match &*local_addrs {
            [first, rest @ ..] => (first, rest),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "no local network interface with private IPv4 address found",
                ));
            }
        };

        // Bind to 0.0.0.0 so that we're available from all IPs the system has.
        let tcp_listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let port = match tcp_listener.local_addr()? {
            SocketAddr::V4(addr) => addr.port(),
            SocketAddr::V6(_) => unreachable!(), // we listened on a V4 address
        };

        let name: Label = format!("providence-{first_addr}")
            .replace('.', "-")
            .parse()
            .unwrap();
        let mut advertiser = AsyncAdvertiser::new(name.clone(), first_addr.into())?;
        for &addr in more_addrs {
            advertiser.add_name(name.clone(), addr.into());
        }
        advertiser.add_instance(
            ServiceInstance::new(name.clone(), Label::new(SERVICE), ServiceTransport::TCP),
            InstanceDetails::new(format!("{name}.local").parse().unwrap(), port),
        );

        let message: Value<Option<Arc<TrackingMessage>>> = Value::new(None);
        let message_reader = message.reader();
        let connections = Value::new(0);
        let connections_reader = connections.reader();
        let advertiser = Task::spawn(async move { advertiser.listen().await });
        let listener = Task::spawn(async move {
            // (contains `Task`s so that they make progress without us polling them)
            let mut streams = Vec::<Task<_>>::new();
            let listener = async_std::net::TcpListener::from(tcp_listener);

            loop {
                let (mut stream, sockaddr) = listener.accept().await?;
                log::info!("client connected: {}", sockaddr);

                // Clean up periodically to avoid unbounded memory growth.
                streams.retain(|task| !task.is_finished());

                let mut message_reader = message_reader.clone();
                let mut connections = connections.clone();
                streams.push(Task::spawn(async move {
                    connections.modify(|mut c| *c += 1);
                    let _fin = defer(|| connections.modify(|mut c| *c -= 1));

                    // If there's an existing message available, send it to the client immediately.
                    if let Ok(Some(msg)) = message_reader.get() {
                        log::debug!("sending existing message to client");
                        msg.async_write(&mut stream).await?;
                    }

                    loop {
                        let msg = match message_reader.wait().await {
                            Ok(Some(msg)) => msg,
                            Ok(None) => continue,
                            Err(_) => break,
                        };
                        msg.async_write(&mut stream).await?;
                    }
                    Ok::<(), io::Error>(())
                }));
            }
        });

        Ok(Self {
            port,
            message,
            connections_reader,
            _advertiser: advertiser,
            _listener: listener,
        })
    }

    /// Updates the [`TrackingMessage`] that is sent to connected clients.
    pub fn publish(&mut self, message: TrackingMessage) {
        self.message.set(Some(Arc::new(message)));
    }

    /// Clears the stored tracking message.
    ///
    /// This prevents any newly connecting client from being served a stale message.
    pub fn clear(&mut self) {
        self.message.set(None);
    }

    /// Returns a [`bool`] indicating whether there is at least 1 client connected to this
    /// [`Publisher`] that would see the published tracking messages.
    pub fn has_connection(&mut self) -> bool {
        self.connections_reader.get().unwrap() != 0
    }

    /// Blocks the calling thread until there is at least 1 client connected to this [`Publisher`].
    pub fn block_until_connected(&mut self) {
        loop {
            if self.connections_reader.get().unwrap() != 0 {
                break;
            }
            log::info!("waiting for connection");
            self.connections_reader.block_until_changed();
        }
    }

    /// Returns the local port the server was bound to.
    #[inline]
    pub fn port(&self) -> u16 {
        self.port
    }
}

pub struct Subscriber {
    task: Option<Task<io::Result<()>>>, // FIXME: ! instead of ()
    reader: Reader<Option<Arc<TrackingMessage>>>,
}

impl Subscriber {
    pub fn autoconnect_blocking() -> io::Result<Self> {
        let service = Service::new(Label::new(SERVICE), ServiceTransport::TCP);
        let mut discoverer = SyncDiscoverer::new_multicast_v4()?;

        let mut instance = None;
        discoverer.discover_instances(&service, |new| {
            instance = Some(new.clone());
            ControlFlow::Break(())
        })?;
        let details = match instance {
            Some(instance) => discoverer.load_instance_details(&instance)?,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out while discovering `{}` network service", SERVICE),
                ));
            }
        };
        log::info!(
            "discovered providence on {}:{}",
            details.host(),
            details.port(),
        );

        let mut res = SyncResolver::new_multicast_v4()?;
        let mut ips = res
            .resolve_domain(details.host())?
            .filter_map(|ip| match ip {
                IpAddr::V4(ip) => Some(ip),
                IpAddr::V6(_) => None,
            });
        let ip = ips.next().ok_or(io::ErrorKind::TimedOut)?;
        log::info!("resolved server IP: {}", ip);

        Self::connect(SocketAddrV4::new(ip, details.port()))
    }

    pub async fn autoconnect_async() -> io::Result<Self> {
        let service = Service::new(Label::new(SERVICE), ServiceTransport::TCP);
        let mut discoverer = AsyncDiscoverer::new_multicast_v4().await?;

        let mut instance = None;
        discoverer.set_discovery_timeout(Duration::MAX)?;
        discoverer
            .discover_instances(&service, |new| {
                instance = Some(new.clone());
                ControlFlow::Break(())
            })
            .await?;
        let details = match instance {
            Some(instance) => discoverer.load_instance_details(&instance).await?,
            None => {
                // The timeout is ~infinite, good luck hitting this
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out while discovering `{}` network service", SERVICE),
                ));
            }
        };
        log::info!(
            "discovered providence on {}:{}",
            details.host(),
            details.port(),
        );

        let mut res = AsyncResolver::new_multicast_v4().await?;
        let mut ips = res
            .resolve_domain(details.host())
            .await?
            .filter_map(|ip| match ip {
                IpAddr::V4(ip) => Some(ip),
                IpAddr::V6(_) => None,
            });
        let ip = ips.next().ok_or(io::ErrorKind::TimedOut)?;
        log::info!("resolved server IP: {}", ip);

        Self::connect(SocketAddrV4::new(ip, details.port()))
    }

    pub fn connect(addr: SocketAddrV4) -> io::Result<Self> {
        let mut message = Value::new(None);
        let reader = message.reader();

        let task = Task::spawn(async move {
            let mut stream = async_std::net::TcpStream::connect(addr).await?;
            log::info!("connected to server at {addr}");
            loop {
                let msg = Arc::new(TrackingMessage::async_read(&mut stream).await?);
                message.set(Some(msg));
            }
        });

        Ok(Self {
            task: Some(task),
            reader,
        })
    }

    /// Retrieves the most recent message received.
    ///
    /// Returns [`None`] if no [`TrackingMessage`] has ever been received by this [`Subscriber`].
    pub fn get(&mut self) -> io::Result<Option<Arc<TrackingMessage>>> {
        match self.reader.get() {
            Ok(opt) => Ok(opt),
            Err(Disconnected) => Err(self.ping().unwrap_err()),
        }
    }

    /// Retrieves the next [`TrackingMessage`] received.
    ///
    /// If no message was received since the last time one was retrieved from this [`Subscriber`],
    /// this function returns [`None`]. If you want to access the last message regardless, call
    /// [`Subscriber::get`] instead.
    pub fn next(&mut self) -> io::Result<Option<Arc<TrackingMessage>>> {
        if self.reader.has_changed() {
            match self.reader.get() {
                Ok(opt) => Ok(Some(opt.unwrap())),
                Err(Disconnected) => Err(self.ping().unwrap_err()),
            }
        } else {
            Ok(None)
        }
    }

    /// Blocks the calling thread until a new [`TrackingMessage`] is available, and returns the
    /// message.
    pub fn block(&mut self) -> io::Result<Arc<TrackingMessage>> {
        // If the writer disconnects, the task must have returned an error or panicked.
        self.reader
            .block()
            .map(Option::unwrap)
            .map_err(|_| self.task.take().unwrap().block().unwrap_err())
    }

    fn ping(&mut self) -> io::Result<()> {
        if self.reader.is_disconnected() {
            Err(self.task.take().unwrap().block().unwrap_err())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{Eye, FaceData, Image, Mesh, PersistentId, Vertex};

    use super::*;

    #[test]
    fn pub_sub_are_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<Publisher>();
        check::<Subscriber>();
    }

    #[test]
    fn publisher_exits() {
        Publisher::spawn().unwrap();
    }

    #[test]
    fn io() {
        env_logger::init();

        let mut p = Publisher::spawn().unwrap();
        p.publish(mk_test_msg());
        // Connect after publishing so that an initial message will be received.
        let mut s = Subscriber::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, p.port())).unwrap();
        s.block().unwrap();
        let _msg = s.get().unwrap();
    }

    fn mk_test_msg() -> TrackingMessage {
        fn mk_eye() -> Eye {
            Eye {
                mesh: Mesh {
                    indices: vec![0, 1, 2],
                    vertices: vec![
                        Vertex {
                            position: [0.0, -1.0, 0.5],
                            uv: [1.0, 0.5],
                        };
                        3
                    ],
                },
                texture: Image {
                    data: vec![0, 1, 2, 3],
                    height: 1,
                    width: 1,
                },
                iris_center: [0.0; 3],
                iris_radius: 0.25,
            }
        }

        TrackingMessage {
            faces: vec![FaceData {
                ephemeral_id: 123,
                persistent_id: PersistentId::Unknown,
                head_position: [1.0, 2.0],
                head_rotation: Default::default(),
                left_eye: mk_eye(),
                right_eye: mk_eye(),
            }],
        }
    }
}
