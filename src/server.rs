use atone::Vc as VecDeque;
use std::{
  convert::AsRef,
  error::Error,
  fmt,
  io::{Error as IoError, ErrorKind as IoErrorKind},
  net::{SocketAddr, UdpSocket},
  ops::Deref,
  sync::Arc,
  time::{Duration, Instant},
};

use async_io::Async;
use futures_util::{pin_mut, select, FutureExt, StreamExt};
use hashbrown::hash_map::{Entry as HashMapEntry, HashMap};
use openssl::ssl::SslAcceptor;
use rand::thread_rng;
use socket2::{Domain, SockAddr, Socket, Type};

use crate::{
  buffer_pool::{BufferHandle, BufferPool, OwnedBuffer},
  client::{Client, ClientError, MessageType, MAX_UDP_PAYLOAD_SIZE},
  crypto::Crypto,
  interval::Interval,
  sdp::{gen_sdp_response, parse_sdp_fields, SdpFields},
  stun::{parse_stun_binding_request, write_stun_success_response},
  util::rand_string,
};

#[derive(Debug)]
pub enum SendError {
  ClientNotConnected,
  IncompleteMessageWrite,
  ClientError(String),
  Io(IoError),
}

impl fmt::Display for SendError {
  fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
    match self {
      SendError::ClientNotConnected => write!(f, "client is not connected"),
      SendError::IncompleteMessageWrite => {
        write!(f, "incomplete write of WebRTC Data Channel message")
      }
      SendError::Io(err) => fmt::Display::fmt(err, f),
      SendError::ClientError(msg) => fmt::Display::fmt(msg, f),
    }
  }
}

impl Error for SendError {}

impl From<IoError> for SendError {
  fn from(err: IoError) -> SendError {
    SendError::Io(err)
  }
}

#[derive(Debug)]
pub enum SessionError {
  /// `SessionEndpoint` has beeen disconnected from its `Server` (the `Server` has been dropped).
  Disconnected,
  /// An error streaming the SDP descriptor
  ParseError(Box<dyn Error + 'static>),
}

impl fmt::Display for SessionError {
  fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
    match self {
      SessionError::Disconnected => write!(f, "`SessionEndpoint` disconnected from `Server`"),
      SessionError::ParseError(e) => {
        write!(f, "error streaming the incoming SDP descriptor: {}", e)
      }
    }
  }
}

impl Error for SessionError {
  fn source(&self) -> Option<&(dyn Error + 'static)> {
    match self {
      SessionError::Disconnected => None,
      SessionError::ParseError(e) => Some(e.as_ref()),
    }
  }
}

/// A reference to an internal buffer containing a received message.
pub struct MessageBuffer<'a>(BufferHandle<'a>);

impl<'a> Deref for MessageBuffer<'a> {
  type Target = Vec<u8>;

  fn deref(&self) -> &Vec<u8> {
    &self.0
  }
}

impl<'a> AsRef<[u8]> for MessageBuffer<'a> {
  fn as_ref(&self) -> &[u8] {
    &self.0
  }
}

///
/// Struct representing a ErrorMessage
///
#[repr(C)]
pub struct ErrorMessage {
  pub code: i32,
  pub message: String,
}
pub struct MessageResult<'a> {
  pub message: MessageBuffer<'a>,
  pub message_type: MessageType,
  pub remote_addr: SocketAddr,
}

#[derive(Clone)]
pub struct SessionEndpoint {
  public_addr: SocketAddr,
  cert_fingerprint: Arc<String>,
  session_sender: flume::Sender<IncomingSession>,
}

impl SessionEndpoint {
  /// Receives an incoming SDP descriptor of an `RTCSessionDescription` from a browser, informs
  /// the corresponding `Server` of the new WebRTC session, and returns a JSON object containing
  /// objects which can construct an `RTCSessionDescription` and an `RTCIceCandidate` in a
  /// browser.
  ///
  /// The returned JSON object contains a digest of the x509 certificate the server will use for
  /// DTLS, and the browser will ensure that this digest matches before starting a WebRTC
  /// connection.
  pub fn session_request(&mut self, sdp_descriptor: &str) -> Result<String, SessionError> {
    const SERVER_USER_LEN: usize = 12;
    const SERVER_PASSWD_LEN: usize = 24;

    let SdpFields { ice_ufrag, mid, .. } =
      parse_sdp_fields(sdp_descriptor).map_err(|e| SessionError::ParseError(e.into()))?;

    let (incoming_session, response) = {
      let mut rng = thread_rng();
      let server_user = rand_string(&mut rng, SERVER_USER_LEN);
      let server_passwd = rand_string(&mut rng, SERVER_PASSWD_LEN);

      let incoming_session = IncomingSession {
        server_user: server_user.clone(),
        server_passwd: server_passwd.clone(),
        remote_user: ice_ufrag,
      };

      let response = gen_sdp_response(
        &mut rng,
        &self.cert_fingerprint,
        &self.public_addr.ip().to_string(),
        self.public_addr.ip().is_ipv6(),
        self.public_addr.port(),
        &server_user,
        &server_passwd,
        &mid,
      );

      (incoming_session, response)
    };

    let incoming_session = incoming_session;
    let handler = self.session_sender.send(incoming_session);
    if handler.is_err() {
      return Err(SessionError::Disconnected);
    }
    Ok(response)
  }
}
pub struct Server {
  udp_socket: Async<UdpSocket>,
  session_endpoint: SessionEndpoint,
  incoming_session_stream: flume::Receiver<IncomingSession>,
  ssl_acceptor: SslAcceptor,
  outgoing_udp: VecDeque<(OwnedBuffer, SocketAddr)>,
  incoming_rtc: VecDeque<(OwnedBuffer, SocketAddr, MessageType)>,
  buffer_pool: BufferPool,
  sessions: HashMap<SessionKey, Session>,
  clients: HashMap<SocketAddr, Client>,
  last_generate_periodic: Instant,
  last_cleanup: Instant,
  periodic_timer: Interval,
}
// unsafe impl Send for Server {}

impl Server {
  /// Start a new WebRTC data channel server listening on `listen_addr` and advertising its
  /// publicly available address as `public_addr`.
  ///
  /// WebRTC connections must be started via an external communication channel from a browser via
  /// the `SessionEndpoint`, after which a WebRTC data channel can be opened.
  pub fn new(
    listen_addr: SocketAddr,
    public_addr: SocketAddr,
    cb: Option<extern "C" fn(u32, *mut u8, u32)>,
  ) -> Result<Server, IoError> {
    const SESSION_BUFFER_SIZE: usize = 8;
    if cb.is_some() {
      unsafe {
        EVENT_CB = cb;
      }
    }
    let crypto = Crypto::init().expect("WebRTC server could not initialize OpenSSL primitives");

    let inner = Socket::new(Domain::for_address(listen_addr), Type::DGRAM, None).unwrap();

    //This is temporary disable due to probleme with Sessions management.
    //the sessions should be handled in the Deno side using a single UDP socket and a Map to store each request,
    //then wait until we get a new UDP connection in Rust side to handle the DTLS part.

    // #[cfg(any(unix))]
    // inner.set_reuse_port(true).unwrap();

    // inner.set_reuse_address(true).unwrap();

    let address = SockAddr::from(listen_addr);
    inner.bind(&address)?;

    let sock = inner.into();

    let udp_socket = Async::new(sock)?;
    let (session_sender, session_receiver) = flume::bounded(SESSION_BUFFER_SIZE);

    let session_endpoint = SessionEndpoint {
      public_addr,
      cert_fingerprint: Arc::new(crypto.fingerprint),
      session_sender,
    };

    Ok(Server {
      udp_socket,
      session_endpoint,
      incoming_session_stream: session_receiver,
      ssl_acceptor: crypto.ssl_acceptor,
      outgoing_udp: VecDeque::new(),
      incoming_rtc: VecDeque::new(),
      buffer_pool: BufferPool::new(),
      sessions: HashMap::new(),
      clients: HashMap::new(),
      last_generate_periodic: Instant::now(),
      last_cleanup: Instant::now(),
      periodic_timer: Interval::new(PERIODIC_TIMER_INTERVAL),
    })
  }
  /// Returns a `SessionEndpoint` which can be used to start new WebRTC sessions.
  ///
  /// WebRTC connections must be started via an external communication channel from a browser via
  /// the returned `SessionEndpoint`, and this communication channel will be used to exchange
  /// session descriptions in SDP format.
  ///
  /// The returned `SessionEndpoint` will notify this `Server` of new sessions via a shared async
  /// channel.  This is done so that the `SessionEndpoint` is easy to use in a separate server
  /// task (such as a `hyper` HTTP server).
  pub fn session_endpoint(&self) -> SessionEndpoint {
    self.session_endpoint.clone()
  }

  /// The total count of clients in any active state, whether still starting up, fully
  /// established, or still shutting down.
  pub fn active_clients(&self) -> usize {
    self.clients.values().filter(|c| !c.is_shutdown()).count()
  }

  /// List all the currently fully established client connections.
  pub fn connected_clients(&mut self) -> String {
    self
      .clients
      .iter_mut()
      .filter(|(_, c)| c.is_established())
      .map(|(addr, _)| addr.to_string() + ",")
      .collect::<String>()
  }

  /// Returns true if the client has a completely established WebRTC data channel connection and
  /// can send messages back and forth.  Returns false for disconnected clients as well as those
  /// that are still starting up or are in the process of shutting down.
  pub fn is_connected(&self, remote_addr: &SocketAddr) -> bool {
    if let Some(client) = self.clients.get(remote_addr) {
      client.is_established()
    } else {
      false
    }
  }

  /// Disconect the given client, does nothing if the client is not currently connected.
  pub async fn disconnect(&mut self, remote_addr: &SocketAddr) -> Result<(), IoError> {
    if let Some(client) = self.clients.get_mut(remote_addr) {
      match client.start_shutdown() {
        Ok(true) => {
          //   log::info!("starting shutdown for client {}", remote_addr);
        }
        Ok(false) => {}
        Err(_) => {}
      }

      self
        .outgoing_udp
        .extend(client.take_outgoing_packets().map(|p| (p, *remote_addr)));
      match self.send_outgoing().await {
        Ok(_) => {}
        Err(_) => {}
      }
    }

    Ok(())
  }

  /// Send the given message to the given remote client, if they are connected.
  ///
  /// The given message must be less than `MAX_MESSAGE_LEN`.
  pub async fn send(
    &mut self,
    message: &[u8],
    message_type: MessageType,
    remote_addr: &SocketAddr,
  ) -> Result<(), SendError> {
    let client = self
      .clients
      .get_mut(remote_addr)
      .ok_or(SendError::ClientNotConnected)?;

    let send_result = client.send_message(message_type, message);
    match send_result {
      Err(ClientError::NotConnected) | Err(ClientError::NotEstablished) => {
        return Err(SendError::ClientNotConnected).into();
      }
      Err(ClientError::IncompletePacketWrite) => {
        return Err(SendError::IncompleteMessageWrite).into();
      }
      Err(err) => {
        let shutdown = client.start_shutdown();
        let catcher = match shutdown {
          Ok(true) => Err(SendError::ClientError(err.to_string())),
          Ok(false) => Err(SendError::ClientNotConnected),
          Err(cerror) => Err(SendError::ClientError(cerror.to_string())),
        };
        return catcher.into();
      }
      Ok(()) => {}
    }

    self
      .outgoing_udp
      .extend(client.take_outgoing_packets().map(|p| (p, *remote_addr)));
    self.send_outgoing().await?;
    Ok(())
  }

  /// Receive a WebRTC data channel message from any connected client.
  ///
  /// `Server::recv` *must* be called for proper operation of the server, as it also handles
  /// background tasks such as responding to STUN packets and timing out existing sessions.
  ///
  /// If the provided buffer is not large enough to hold the received message, the received
  /// message will be truncated, and the original length will be returned as part of
  /// `MessageResult`.
  pub async fn recv(&mut self) -> Result<MessageResult<'_>, IoError> {
    while self.incoming_rtc.is_empty() {
      self.process().await?;
    }

    let (message, remote_addr, message_type) = self.incoming_rtc.pop_front().unwrap();
    return Ok(MessageResult {
      message: MessageBuffer(self.buffer_pool.adopt(message)),
      message_type,
      remote_addr,
    });
  }
  // Accepts new incoming WebRTC sessions, times out existing WebRTC sessions, sends outgoing UDP
  // packets, receives incoming UDP packets, and responds to STUN packets.
  async fn process(&mut self) -> Result<(), IoError> {
    enum Next {
      IncomingSession(IncomingSession),
      IncomingPacket(usize, SocketAddr),
      PeriodicTimer,
    }

    let mut packet_buffer = self.buffer_pool.acquire();
    packet_buffer.resize(MAX_UDP_PAYLOAD_SIZE, 0);
    let next = {
      let recv_udp = self.udp_socket.recv_from(&mut packet_buffer).fuse();
      pin_mut!(recv_udp);

      let timer_next = self.periodic_timer.next().fuse();
      pin_mut!(timer_next);

      select! {
        incoming_session = self.incoming_session_stream.recv_async().fuse() => {
          Next::IncomingSession(incoming_session.expect("connection to SessionEndpoint has closed"))
        }
        res = recv_udp => {
          let (len, remote_addr) = res?;
          Next::IncomingPacket(len, remote_addr)
        }
        _ = timer_next => {
          Next::PeriodicTimer
        }
      }
    };

    match next {
      Next::IncomingSession(incoming_session) => {
        drop(packet_buffer);
        self.accept_session(incoming_session)
      }
      Next::IncomingPacket(len, remote_addr) => {
        if len > MAX_UDP_PAYLOAD_SIZE {
          return Err(IoError::new(
            IoErrorKind::Other,
            "failed to read entire datagram from socket",
          ));
        }
        packet_buffer.truncate(len);
        let packet_buffer = packet_buffer.into_owned();
        self.receive_packet(remote_addr, packet_buffer);
        self.send_outgoing().await?;
      }
      Next::PeriodicTimer => {
        drop(packet_buffer);
        self.timeout_clients();
        self.generate_periodic_packets();
        self.send_outgoing().await?;
      }
    }

    Ok(())
  }

  // Send all pending outgoing UDP packets
  async fn send_outgoing(&mut self) -> Result<(), IoError> {
    while let Some((packet, remote_addr)) = self.outgoing_udp.pop_front() {
      let packet = self.buffer_pool.adopt(packet);
      let len = self.udp_socket.send_to(&packet, remote_addr).await?;
      let packet_len = packet.len();
      if len != packet_len {
        return Err(IoError::new(
          IoErrorKind::Other,
          "failed to write entire datagram to socket",
        ));
      }
    }
    Ok(())
  }

  // Handle a single incoming UDP packet, either by responding to it as a STUN binding request or
  // by handling it as part of an existing WebRTC connection.
  fn receive_packet(&mut self, remote_addr: SocketAddr, packet_buffer: OwnedBuffer) {
    let mut packet_buffer = self.buffer_pool.adopt(packet_buffer);
    if let Some(stun_binding_request) = parse_stun_binding_request(&packet_buffer[..]) {
      if let Some(session) = self.sessions.get_mut(&SessionKey {
        server_user: stun_binding_request.server_user,
        remote_user: stun_binding_request.remote_user,
      }) {
        session.ttl = Instant::now();
        packet_buffer.resize(MAX_UDP_PAYLOAD_SIZE, 0);
        let resp_len = write_stun_success_response(
          stun_binding_request.transaction_id,
          remote_addr,
          session.server_passwd.as_bytes(),
          &mut packet_buffer,
        );
        match resp_len {
          Ok(len) => {
            packet_buffer.truncate(len);
            self
              .outgoing_udp
              .push_back((packet_buffer.into_owned(), remote_addr));

            match self.clients.entry(remote_addr) {
              HashMapEntry::Vacant(vacant) => {
                let client = Client::new(
                  &self.ssl_acceptor,
                  self.buffer_pool.clone(),
                  remote_addr,
                  unsafe { EVENT_CB },
                );
                match client {
                  Ok(cl) => {
                    vacant.insert(cl);
                  }
                  Err(err) => unsafe {
                    let mut msg = err.to_string();
                    EVENT_CB.as_mut().unwrap()(0, msg.as_mut_ptr(), msg.len() as u32)
                  },
                }
              }
              HashMapEntry::Occupied(_) => {}
            }
          }
          Err(_) => {}
        };
      }
    } else {
      if let Some(client) = self.clients.get_mut(&remote_addr) {
        let client = client;
        if let Err(_err) = client.receive_incoming_packet(packet_buffer.into_owned()) {
          if !client.shutdown_started() {
            let _ = client.start_shutdown();
          }
        }
        let outgoing_packets = client.take_outgoing_packets();
        self
          .outgoing_udp
          .extend(outgoing_packets.map(|p| (p, remote_addr)));
        let incoming_messages = client.receive_messages();
        self.incoming_rtc.extend(
          incoming_messages.map(|(message_type, message)| (message, remote_addr, message_type)),
        );
      }
    }
  }

  // Call `Client::generate_periodic` on all clients, if we are due to do so.
  fn generate_periodic_packets(&mut self) {
    if self.last_generate_periodic.elapsed() >= PERIODIC_PACKET_INTERVAL {
      self.last_generate_periodic = Instant::now();

      for (remote_addr, client) in &mut self.clients {
        if let Err(_err) = client.generate_periodic() {
          if !client.shutdown_started() {
            let _ = client.start_shutdown();
          }
        }
        self
          .outgoing_udp
          .extend(client.take_outgoing_packets().map(|p| (p, *remote_addr)));
      }
    }
  }

  // Clean up all client sessions / connections, if we are due to do so.
  fn timeout_clients(&mut self) {
    if self.last_cleanup.elapsed() >= CLEANUP_INTERVAL {
      self.last_cleanup = Instant::now();
      self.sessions.retain(|_session_key, session| {
        if session.ttl.elapsed() < RTC_SESSION_TIMEOUT {
          true
        } else {
          false
        }
      });

      self.clients.retain(|remote_addr, client| {
        if !client.is_shutdown() && client.last_activity().elapsed() < RTC_CONNECTION_TIMEOUT {
          true
        } else {
          if !client.shutdown_started() {
            unsafe {
              let mut msg = format!("{}:{}", remote_addr.ip(), remote_addr.port());
              EVENT_CB.unwrap()(1002, msg.as_mut_ptr(), msg.len() as u32);
            }
          }
          false
        }
      });
    }
  }

  fn accept_session(&mut self, incoming_session: IncomingSession) {
    self.sessions.insert(
      SessionKey {
        server_user: incoming_session.server_user,
        remote_user: incoming_session.remote_user,
      },
      Session {
        server_passwd: incoming_session.server_passwd,
        ttl: Instant::now(),
      },
    );
  }
  pub fn shutdown_started(&self, remote_addr: &SocketAddr) -> Option<bool> {
    if let Some(client) = self.clients.get(remote_addr) {
      Some(client.shutdown_started())
    } else {
      None
    }
  }
  pub fn client_activity(&mut self, addr: &SocketAddr) -> Option<(u128, u128, u128)> {
    if let Some(client) = self.clients.get_mut(addr) {
      Some((
        client.client_state.last_activity.elapsed().as_millis(),
        client.client_state.last_sent.elapsed().as_millis(),
        client.client_state.last_received.elapsed().as_millis(),
      ))
    } else {
      None
    }
  }
  /// Shutdown the whole server, clear sessions and clients.
  ///
  pub fn shutdown(&mut self) {
    for client in self.clients.values_mut() {
      let _ = client.start_shutdown();
    }
    self.clients.clear();
    self.sessions.clear();
    drop(self.udp_socket.as_ref());
  }
}

const RTC_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const RTC_SESSION_TIMEOUT: Duration = Duration::from_secs(30);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(10);
const PERIODIC_PACKET_INTERVAL: Duration = Duration::from_secs(1);
const PERIODIC_TIMER_INTERVAL: Duration = Duration::from_secs(1);
pub static mut EVENT_CB: Option<extern "C" fn(u32, *mut u8, u32)> = None;

#[derive(Eq, PartialEq, Hash, Clone, Debug)]
struct SessionKey {
  server_user: String,
  remote_user: String,
}

struct Session {
  server_passwd: String,
  ttl: Instant,
}

struct IncomingSession {
  pub server_user: String,
  pub server_passwd: String,
  pub remote_user: String,
}
