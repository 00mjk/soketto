// Example to be used with the autobahn test suite, a fully automated test
// suite to verify client and server implementations of websocket
// implementation.
//
// Once started, the tests can be executed with: wstest -m fuzzingserver
//
// See https://github.com/crossbario/autobahn-testsuite for details.

use futures::{future::{self, Either}, prelude::*};
use log::debug;
use std::{borrow::Cow, error, io, str::FromStr};
use tokio::codec::{Framed, FramedParts};
use tokio::net::TcpStream;
use twist::{base, handshake, Connection, Mode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let n = num_of_cases()?;
    for i in 1 ..= n {
        if let Err(e) = run_case(i) {
            debug!("case {}: {:?}", i, e)
        }
    }
    update_report()?;
    Ok(())
}

fn num_of_cases() -> Result<usize, Box<dyn error::Error>> {
    let addr = "127.0.0.1:9001".parse().unwrap();
    TcpStream::connect(&addr)
        .map_err(|e| Box::new(e) as Box<dyn error::Error>)
        .and_then(|socket| {
            let host = Cow::Owned("127.0.0.1:9001".into());
            let resource = Cow::Owned("/getCaseCount".into());
            let client = handshake::Client::new(host, resource);
            tokio::codec::Framed::new(socket, client)
                .send(())
                .map_err(|e| Box::new(e) as Box<dyn error::Error>)
                .and_then(|framed| {
                    framed.into_future().map_err(|(e, _)| Box::new(e) as Box<dyn error::Error>)
                })
                .and_then(|(response, framed)| {
                    if response.is_none() {
                        let e: io::Error = io::ErrorKind::ConnectionAborted.into();
                        return Either::A(future::err(Box::new(e) as Box<dyn error::Error>))
                    }
                    let framed = {
                        let codec = base::Codec::new();
                        let old = framed.into_parts();
                        let mut new = FramedParts::new(old.io, codec);
                        new.read_buf = old.read_buf;
                        new.write_buf = old.write_buf;
                        let framed = Framed::from_parts(new);
                        Connection::from_framed(framed, Mode::Client)
                    };
                    Either::B(framed.into_future().map_err(|(e, _)| Box::new(e) as Box<dyn error::Error>))
                })
                .and_then(|(data, _framed)| {
                    let bytes = match data {
                        Some(base::Data::Binary(b)) => b,
                        Some(base::Data::Text(b)) => b,
                        None => {
                            let e: io::Error = io::ErrorKind::ConnectionAborted.into();
                            return Either::A(future::err(Box::new(e) as Box<dyn error::Error>))
                        }
                    };
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        return Either::B(future::ok(usize::from_str(s).unwrap_or(0)))
                    }
                    let e = io::Error::new(io::ErrorKind::Other, "invalid payload");
                    Either::A(future::err(Box::new(e) as Box<dyn error::Error>))
                })
        })
        .wait()
}

fn run_case(n: usize) -> Result<(), Box<dyn error::Error>> {
    let addr = "127.0.0.1:9001".parse().unwrap();
    TcpStream::connect(&addr)
        .map_err(|e| Box::new(e) as Box<dyn error::Error>)
        .and_then(move |socket| {
            let host = Cow::Owned("127.0.0.1:9001".into());
            let resource = Cow::Owned(format!("/runCase?case={}&agent=foo", n));
            let client = handshake::Client::new(host, resource);
            tokio::codec::Framed::new(socket, client)
                .send(())
                .map_err(|e| Box::new(e) as Box<dyn error::Error>)
                .and_then(|framed| {
                    framed.into_future().map_err(|(e, _)| Box::new(e) as Box<dyn error::Error>)
                })
                .and_then(|(response, framed)| {
                    if response.is_none() {
                        let e: io::Error = io::ErrorKind::ConnectionAborted.into();
                        return Either::A(future::err(Box::new(e) as Box<dyn error::Error>))
                    }
                    let connection = {
                        let codec = base::Codec::new();
                        let old = framed.into_parts();
                        let mut new = FramedParts::new(old.io, codec);
                        new.read_buf = old.read_buf;
                        new.write_buf = old.write_buf;
                        let framed = Framed::from_parts(new);
                        Connection::from_framed(framed, Mode::Client)
                    };
                    Either::B(future::ok(connection))
                })
                .and_then(|connection| {
                    let (sink, stream) = connection.split();
                    let sink = sink.with(|data: base::Data| {
                        if data.is_text() {
                            std::str::from_utf8(data.as_ref())?;
                        }
                        Ok(data)
                    });
                    stream.forward(sink)
                        .and_then(|(_stream, mut sink)| future::poll_fn(move || sink.close()))
                        .map_err(|e| Box::new(e) as Box<dyn error::Error>)
                })
        })
        .wait()
}

fn update_report() -> Result<(), Box<dyn error::Error>> {
    let addr = "127.0.0.1:9001".parse().unwrap();
    TcpStream::connect(&addr)
        .map_err(|e| Box::new(e) as Box<dyn error::Error>)
        .and_then(|socket| {
            let host = Cow::Owned("127.0.0.1:9001".into());
            let resource = Cow::Owned("/updateReports?agent=foo".into());
            let client = handshake::Client::new(host, resource);
            tokio::codec::Framed::new(socket, client)
                .send(())
                .map_err(|e| Box::new(e) as Box<dyn error::Error>)
                .and_then(|framed| {
                    framed.into_future().map_err(|(e, _)| Box::new(e) as Box<dyn error::Error>)
                })
                .and_then(|(response, framed)| {
                    if response.is_none() {
                        let e: io::Error = io::ErrorKind::ConnectionAborted.into();
                        return Either::A(future::err(Box::new(e) as Box<dyn error::Error>))
                    }
                    let mut framed = {
                        let codec = base::Codec::new();
                        let old = framed.into_parts();
                        let mut new = FramedParts::new(old.io, codec);
                        new.read_buf = old.read_buf;
                        new.write_buf = old.write_buf;
                        let framed = Framed::from_parts(new);
                        Connection::from_framed(framed, Mode::Client)
                    };
                    Either::B(future::poll_fn(move || {
                        framed.close().map_err(|e| Box::new(e) as Box<dyn error::Error>)
                    }))
                })
        })
        .wait()
}