use rand::Rng;
use sdp::SessionDescription;
use std::{error, str};
pub type Error = Box<dyn error::Error>;
use std::io::Cursor;

#[derive(Debug)]
pub struct SdpFields {
  pub ice_ufrag: String,
  pub ice_passwd: String,
  pub mid: String,
}

pub fn parse_sdp_fields(body: &str) -> Result<SdpFields, Error> {
  let mut reader = Cursor::new(body.as_bytes());
  let sdp = SessionDescription::unmarshal(&mut reader);
  let sdp = match sdp {
    Ok(sdp) => {
      let mut found_ice_ufrag = None;
      let mut found_ice_passwd = None;
      let mut found_mid = None;
      let media = sdp.media_descriptions;
      for attr in media {
        found_ice_ufrag = attr.attribute("ice-ufrag").unwrap().map(|s| s.to_string());
        found_ice_passwd = attr.attribute("ice-pwd").unwrap().map(|s| s.to_string());
        found_mid = attr.attribute("mid").unwrap().map(|s| s.to_string());
      }
      match (found_ice_ufrag, found_ice_passwd, found_mid) {
        (Some(ice_ufrag), Some(ice_passwd), Some(mid)) => Ok(SdpFields {
          ice_ufrag,
          ice_passwd,
          mid,
        }),
        _ => Err("not all SDP fields provided".into()),
      }
    }
    Err(e) => return Err(Box::new(e)),
  };
  sdp
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
         o=- {rand1} 1 IN {ipv} {port}\\r\\n\
         s=-\\r\\n\
         c=IN {ipv} {ip}\\r\\n\
         t=0 0\\r\\n\
         a=ice-lite\\r\\n\
         a=ice-ufrag:{ufrag}\\r\\n\
         a=ice-pwd:{pass}\\r\\n\
         m=application {port} UDP/DTLS/SCTP webrtc-datachannel\\r\\n\
         a=fingerprint:sha-256 {fingerprint}\\r\\n\
         a=ice-options:trickle\\r\\n\
         a=setup:passive\\r\\n\
         a=mid:{mid}\\r\\n\
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
