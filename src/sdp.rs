use rand::Rng;
use std::{error, str};
pub type Error = Box<dyn error::Error>;

#[derive(Debug)]
pub struct SdpFields {
  pub ice_ufrag: String,
  pub ice_passwd: String,
  pub mid: String,
}

pub fn parse_sdp_fields(body: &str) -> Result<SdpFields, Error> {
  //ice-ufrag
  //ice-pwd
  //a=mid:
  //find the three fields in the string body
  let mut ice_ufrag = String::new();
  let mut ice_passwd = String::new();
  let mut mid = String::new();
  let mut lines = body.lines();
  while let Some(line) = lines.next() {
    if line.starts_with("a=ice-ufrag:") {
      ice_ufrag = line[12..].to_string();
    } else if line.starts_with("a=ice-pwd:") {
      ice_passwd = line[10..].to_string();
    } else if line.starts_with("a=mid:") {
      mid = line[6..].to_string();
    }
  }
  if ice_ufrag.is_empty() || ice_passwd.is_empty() || mid.is_empty() {
    return Err("missing ice-ufrag, ice-pwd, or mid".into());
  }
  Ok(SdpFields {
    ice_ufrag,
    ice_passwd,
    mid,
  })
}

pub fn gen_sdp_response<R: Rng>(
  rng: &mut R,
  cert_fingerprint: &str,
  server_ip: &str,
  server_is_ipv6: bool,
  server_port: u16,
  ufrag: &str,
  pass: &str,
  remote_mid: &str,
) -> String {
  format!(
    "{{\"answer\":{{\"sdp\":\"v=0\\r\\n\
         o=FTL {rand1} 1 IN {ipv} {ip}\\r\\n\
         s=-\\r\\n\
         c=IN {ipv} {ip}\\r\\n\
         t=0 0\\r\\n\
         a=ice-lite\\r\\n\
         a=ice-ufrag:{ufrag}\\r\\n\
         a=ice-pwd:{pass}\\r\\n\
         m=application {port} UDP/DTLS/SCTP webrtc-datachannel\\r\\n\
         a=max-message-size:1160\\r\\n\
         a=fingerprint:sha-256 {fingerprint}\\r\\n\
         a=ice-options:trickle\\r\\n\
         a=setup:passive\\r\\n\
         a=mid:{mid}\\r\\n\
		     a=sctpmap:{port} webrtc-datachannel 8000\\r\\n\
         a=max-message-size:1160\\r\\n\
         a=sendrecv\\r\\n\
         a=sctp-port:{port}\\r\\n\",\
         \"type\":\"answer\"}},\"candidate\":{{\"sdpMLineIndex\":0,\
         \"sdpMid\":\"{mid}\",\"candidate\":\"candidate:1 1 UDP {rand2} {ip} {port} \
         typ host\"}}}}",
    rand1 = rng.gen::<u32>(),
    rand2 = rng.gen::<u32>(),
    fingerprint = cert_fingerprint,
    ip = server_ip,
    port = server_port,
    ufrag = ufrag,
    pass = pass,
    mid = remote_mid,
    ipv = if server_is_ipv6 { "IP6" } else { "IP4" },
  )
}
