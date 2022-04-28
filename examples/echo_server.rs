use clap::{Arg, Command};
use webrtc_unreliable::Server as RtcServer;

#[tokio::main]
async fn main() {
  env_logger::init_from_env(env_logger::Env::new().default_filter_or("debug"));

  let matches = Command::new("echo_server")
    .arg(
      Arg::new("data")
        .short('d')
        .long("data")
        .takes_value(true)
        .required(true)
        .help("listen on the specified address/port for UDP WebRTC data channels"),
    )
    .arg(
      Arg::new("public")
        .short('p')
        .long("public")
        .takes_value(true)
        .required(true)
        .help("advertise the given address/port as the public WebRTC address/port"),
    )
    .arg(
      Arg::new("sdp")
        .short('s')
        .long("sdp")
        .takes_value(true)
        .required(true)
        .help("SDP test"),
    )
    .get_matches();

  let webrtc_listen_addr = matches
    .value_of("data")
    .unwrap()
    .parse()
    .expect("could not parse WebRTC data address/port");

  let public_webrtc_addr = matches
    .value_of("public")
    .unwrap()
    .parse()
    .expect("could not parse advertised public WebRTC data address/port");

  let sdp = matches.value_of("sdp").unwrap();

  let mut rtc_server =
    RtcServer::new(webrtc_listen_addr, public_webrtc_addr).expect("could not start RTC server");

  let mut session_endpoint = rtc_server.session_endpoint();
  match session_endpoint.session_request(sdp) {
    Ok(session) => {
      println!("Copy this SDP to the client: {}", session);
    }
    Err(e) => {
      println!("session failed: {}", e);
    }
  }

  let mut message_buf = Vec::new();
  loop {
    let received = match rtc_server.recv() {
      Ok(received) => {
        message_buf.clear();
        message_buf.extend(received.message.as_ref());
        Some((received.message_type, received.remote_addr))
      }
      Err(_err) => None,
    };

    if let Some((message_type, remote_addr)) = received {
      if let Err(_err) = rtc_server.send(&message_buf, message_type, &remote_addr) {
        // log::warn!("could not send message to {}: {:?}", remote_addr, err);
      }
    }
  }
}
