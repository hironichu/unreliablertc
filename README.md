## webrtc-unreliable

---

This is a Rust library which allows you to write a game server with browser
based clients and UDP-like networking.

...
More of this README in the original Repo [here](https://github.com/kyren/webrtc-unreliable)
...
## Running the example

In a terminal: 
> NOTE FROM THIS FORK

You can start the server using this command, the server will be listening on port 5050, the web page will wait for you to click on start SDP, and the Offer will be displayed in the console, copy this SDP into 
```
$ deno run -A examples/deno.ts
```
Then you can input this Offer into this command --sdp parameter
```
$ cargo run --example echo_server -- --data 127.0.0.1:42424  --public 127.0.0.1:42424 --sdp "<insert the sdp you get from your client>"
```

Please note that if you are using Firefox, Firefox does not accept WebRTC
connections to 127.0.0.1, you need to use your Computer's IP address instead. (e.g. 192.168.1.x)

## Credit
This crate is a fork and was primarely written by [@kyren](https://github.com/kyren)


Also, this was originally a Rust / Tokio port of the
[WebUDP](https://github.com/seemk/WebUdp) project, so the credit for the
original design goes there.

## License

This project is licensed under the [MIT license](LICENSE)
