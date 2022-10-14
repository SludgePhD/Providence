use std::{
    io::{self, BufReader},
    net::{IpAddr, TcpListener, TcpStream},
    ops::ControlFlow,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

use uwuhi::{
    packet::name::Label,
    resolver::Resolver,
    service::{
        advertising::ServiceAdvertiser, discovery::SimpleDiscoverer, InstanceDetails, Service,
        ServiceInstance, ServiceTransport,
    },
};

use crate::data::TrackingMessage;

const SERVICE: &str = "_providence";

pub struct Publisher {
    streams: Arc<Mutex<Vec<(Arc<TcpStream>, JoinHandle<io::Result<()>>)>>>,
    data: Arc<(Mutex<Option<Arc<TrackingMessage>>>, Condvar)>,

    advertiser: Arc<ServiceAdvertiser>,
    tcp_listener: Arc<TcpListener>,
    mdns_thread: Option<JoinHandle<io::Result<()>>>,
    listener_thread: Option<JoinHandle<io::Result<()>>>,
}

impl Publisher {
    pub fn spawn() -> io::Result<Self> {
        // FIXME: there doesn't seem to be a good way to find the default interface/IP address that 0.0.0.0 binds to
        let local_addrs = if_addrs::get_if_addrs()?
            .into_iter()
            .filter_map(|interface| match interface.ip() {
                IpAddr::V4(ip) if ip.is_private() => Some(ip),
                _ => None,
            })
            .collect::<Vec<_>>();

        let addr = match &*local_addrs {
            [ip] => *ip,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!(
                        "need exactly one local address, found {}: {:?}",
                        local_addrs.len(),
                        local_addrs
                    ),
                ))
            }
        };

        let tcp_listener = Arc::new(TcpListener::bind((addr, 0))?);
        let local_addr = tcp_listener.local_addr()?;

        let name: Label = format!("providence-{addr}")
            .replace('.', "-")
            .parse()
            .unwrap();
        let mut advertiser = ServiceAdvertiser::new(name.clone(), addr)?;
        advertiser.add_instance(
            ServiceInstance::new(name.clone(), Label::new(SERVICE), ServiceTransport::TCP),
            InstanceDetails::new(format!("{name}.local").parse().unwrap(), local_addr.port()),
        );
        let advertiser = Arc::new(advertiser);
        let streams = Arc::new(Mutex::new(Vec::new()));
        let data = Arc::new((Mutex::new(None), Condvar::new()));
        Ok(Self {
            streams: streams.clone(),
            data: data.clone(),
            advertiser: advertiser.clone(),
            tcp_listener: tcp_listener.clone(),
            mdns_thread: Some(thread::Builder::new().spawn(move || advertiser.listen())?),
            listener_thread: Some(thread::Builder::new().spawn(move || loop {
                let (stream, sockaddr) = tcp_listener.accept()?;
                log::info!("client connected: {}", sockaddr);

                let stream_arc = Arc::new(stream);
                let stream = stream_arc.clone();
                let data = data.clone();
                let thread = thread::Builder::new()
                    .spawn(move || {
                        let mut stream = &*stream;

                        // If there's an old message available, send it to the client immediately.
                        let guard = data.0.lock().unwrap();
                        if let Some(msg) = &*guard {
                            let msg = msg.clone();
                            drop(guard);

                            msg.write(&mut stream)?;
                        }

                        loop {
                            let guard = data.1.wait(data.0.lock().unwrap()).unwrap();
                            if let Some(msg) = &*guard {
                                let msg = msg.clone();
                                drop(guard);

                                msg.write(&mut &*stream)?;
                            }
                        }
                    })
                    .unwrap();

                streams.lock().unwrap().push((stream_arc, thread));
            })?),
        })
    }

    pub fn publish(&mut self, message: TrackingMessage) {
        let mut guard = self.data.0.lock().unwrap();
        *guard = Some(Arc::new(message));
        self.data.1.notify_all();
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        // FIXME: make all threads exit, welp
        let _ = self.streams;
        let _ = self.advertiser;
        let _ = self.listener_thread;
        let _ = self.mdns_thread;
        let _ = self.tcp_listener;
    }
}

pub struct Subscriber {
    handle: JoinHandle<io::Result<()>>,
    data: Arc<(Mutex<Option<(Arc<TrackingMessage>, u64)>>, Condvar)>,
    last_gen: u64,
}

impl Subscriber {
    pub fn spawn_blocking() -> io::Result<Self> {
        let service = Service::new(Label::new(SERVICE), ServiceTransport::TCP);

        let mut browser = SimpleDiscoverer::new_multicast_v4()?;

        let mut instance = None;
        browser.discover_instances(&service, |new| {
            instance = Some(new.clone());
            ControlFlow::Break(())
        })?;
        let details = match instance {
            Some(instance) => browser.load_instance_details(&instance)?,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out while discovering `{}` network service", SERVICE),
                ))
            }
        };
        log::info!(
            "discovered providence on {}:{}",
            details.host(),
            details.port()
        );

        // Use uwuhi mDNS resolver, since system might not be configured to resolve via mDNS.
        let mut client = Resolver::new_multicast_v4()?;
        let mut ips = client.resolve_domain(details.host())?;
        let ip = ips.next().ok_or(io::ErrorKind::TimedOut)?;
        log::info!("resolved server IP: {}", ip);

        let mut stream = BufReader::new(TcpStream::connect((ip, details.port()))?);
        let data = Arc::new((Mutex::new(None), Condvar::new()));
        let handle = thread::Builder::new().spawn({
            let data = data.clone();
            move || {
                let mut gen = 0;
                loop {
                    let msg = Arc::new(TrackingMessage::read(&mut stream)?);
                    let mut guard = data.0.lock().unwrap();
                    *guard = Some((msg, gen));
                    data.1.notify_all();
                    drop(guard);
                    gen += 1;
                }
            }
        })?;
        Ok(Self {
            handle,
            data,
            last_gen: u64::MAX,
        })
    }

    /// Returns the next [`TrackingMessage`] received, without blocking.
    ///
    /// If no [`TrackingMessage`] has been received since the last call to this method (or the call
    /// to [`Subscriber::spawn_blocking`]), returns [`None`].
    pub fn next(&mut self) -> Option<Arc<TrackingMessage>> {
        let guard = self.data.0.lock().unwrap();
        let (msg, gen) = match &*guard {
            Some(x) => x,
            None => return None,
        };
        if *gen == self.last_gen {
            None
        } else {
            self.last_gen = *gen;
            Some(msg.clone())
        }
    }

    /// Blocks until a new [`TrackingMessage`] has been received.
    ///
    /// When this method returns, the next call to [`Subscriber::next`] is guaranteed to return
    /// [`Some`].
    pub fn block(&self) {
        let mut guard = self.data.0.lock().unwrap();
        loop {
            match &*guard {
                Some((_, gen)) if *gen != self.last_gen => return,
                _ => {}
            }
            guard = self.data.1.wait(guard).unwrap();
        }
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        // TODO: kill the thread
        let _ = self.handle;
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use crate::data::{Eye, Image, Mesh, Vertex};

    use super::*;

    #[test]
    fn publisher_exits() {
        Publisher::spawn().unwrap();
    }

    #[test]
    fn query_port() {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        assert_ne!(addr.port(), 0);
        // Looks like we can query the port the OS picked, but network interface IPs are harder.
    }

    #[test]
    fn io() {
        let mut p = Publisher::spawn().unwrap();
        p.publish(mk_test_msg());
        // Connect after publishing so that an initial message will be received.
        let mut s = Subscriber::spawn_blocking().unwrap();
        s.block();
        let _msg = s.next().unwrap();
    }

    fn mk_test_msg() -> TrackingMessage {
        fn mk_eye() -> Eye {
            Eye {
                mesh: Mesh {
                    indices: vec![0, 1, 2],
                    vertices: vec![
                        Vertex {
                            position: [0.0, -1.0],
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
            }
        }

        TrackingMessage {
            left_eye: mk_eye(),
            right_eye: mk_eye(),
        }
    }
}
