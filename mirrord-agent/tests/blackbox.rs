#[cfg(test)]
mod tests {
    use std::{io::ErrorKind, net::IpAddr, process::Stdio, sync::Arc};

    use actix_codec::Framed;
    use futures::SinkExt;
    use mirrord_protocol::{
        ClientCodec, ClientMessage, DaemonMessage, NewTCPConnection, TCPClose, TCPData,
    };
    use test_bin::get_test_bin;
    use tokio::{
        io::AsyncWriteExt,
        net::{TcpListener, TcpStream},
        select,
        sync::Mutex,
        time::{sleep, Duration},
    };
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn sanity() {
        let mut bin = get_test_bin("mirrord-agent");
        let child = bin
            .arg("-t")
            .arg("2")
            .arg("-i")
            .arg("lo")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("mirrord-agent failed to start");
        // Wait for agent to listen
        sleep(Duration::from_millis(2000)).await;
        let stream = TcpStream::connect("127.0.0.1:61337")
            .await
            .expect("connection to agent failed");
        let mutex = Arc::new(Mutex::new(0));
        let task_mutex = Arc::clone(&mutex);
        let guard = mutex.lock().await;
        let task = tokio::spawn(async move {
            let listener = TcpListener::bind("127.0.0.1:1337")
                .await
                .expect("couldn't bind socket");
            loop {
                select! {
                    Ok((socket, _)) = listener.accept() => {
                        let mut buf = [0; 4096];
                        loop {
                            match socket.try_read(&mut buf) {
                                Ok(0) => break,
                                Ok(_) => {}
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    sleep(Duration::from_millis(10)).await;
                                }
                                Err(e) => panic!("socket error {:?}", e)
                            }
                        }
                    },
                    _ = task_mutex.lock() => {
                        break
                    }
                }
            }
        });

        let mut codec = Framed::new(stream, ClientCodec::new());

        codec
            .send(ClientMessage::PortSubscribe(vec![1337, 1338]))
            .await
            .expect("port subscribe failed");
        // Let message be acknowledged and dummy socket to start listening
        sleep(Duration::from_millis(2000)).await;
        let mut test_conn = TcpStream::connect("127.0.0.1:1337")
            .await
            .expect("connection to dummy failed");
        let port = test_conn.local_addr().unwrap().port();
        let test_data = [0, 3, 5];
        test_conn
            .write(&test_data)
            .await
            .expect("couldn't write test data");
        drop(test_conn);
        let new_conn_msg = codec
            .next()
            .await
            .expect("couldn't get next message")
            .expect("got invalid message");
        let data_msg = codec
            .next()
            .await
            .expect("couldn't get next message")
            .expect("got invalid message");
        let close_msg = codec
            .next()
            .await
            .expect("couldn't get next message")
            .expect("got invalid message");
        assert_eq!(
            new_conn_msg,
            DaemonMessage::NewTCPConnection(NewTCPConnection {
                connection_id: 0,
                address: IpAddr::V4("127.0.0.1".parse().unwrap()),
                destination_port: 1337,
                source_port: port
            })
        );

        assert_eq!(
            data_msg,
            DaemonMessage::TCPData(TCPData {
                connection_id: 0,
                data: test_data.to_vec()
            })
        );

        assert_eq!(
            close_msg,
            DaemonMessage::TCPClose(TCPClose { connection_id: 0 })
        );
        drop(codec);
        drop(guard);
        drop(mutex);
        task.await.unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        print!("{:?}", stderr);
        assert!(!stderr.to_ascii_lowercase().contains("error"));
        assert!(!String::from_utf8_lossy(&result.stdout)
            .to_ascii_lowercase()
            .contains("error"));
    }
}
