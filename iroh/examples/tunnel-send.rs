use std::env;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use iroh::{
    Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, SecretKey,
    endpoint::{presets, QuicTransportConfig},
};
use n0_error::{Result, StdResultExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 自动创建/读取本地持久化二进制密钥文件（32 字节）
async fn load_or_create_key(path: impl AsRef<Path>) -> Result<SecretKey> {
    let path = path.as_ref();
    if path.exists() {
        let bytes = tokio::fs::read(path).await.anyerr()?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "密钥文件损坏，长度必须是 32 字节")
        }).anyerr()?;
        let key = SecretKey::from_bytes(&bytes);
        Ok(key)
    } else {
        let key = SecretKey::generate();
        let bytes = key.to_bytes();
        tokio::fs::write(path, bytes).await.anyerr()?;
        println!("【首次启动】已生成并保存固定的二进制私钥文件至：{:?}", path);
        Ok(key)
    }
}

/// 解析客户端命令行参数
struct ClientArgs {
    server_addr: EndpointAddr,
    expose_local_port: String,
    alpn: Vec<u8>,
}

fn parse_client_args() -> Result<ClientArgs> {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 || args.contains(&"-h".to_string()) || args.contains(&"--help".to_string()) {
        println!("Iroh 端口转发客户端 (生产高能版)");
        println!("用法: cargo run --bin client -- <SERVER_JSON_ADDRESS> [options]");
        println!();
        println!("选项:");
        println!("  -p, --expose-port <port>   本地监听并对外暴露的端口 (默认: 0.0.0.0:15444)");
        println!("  -a, --alpn <alpn_name>     自定义 ALPN 隧道应用协议 (默认: ranyitech.com/port-forwarding/v1)");
        std::process::exit(0);
    }

    let addr_json = &args[1];
    let server_addr: EndpointAddr = serde_json::from_str(addr_json).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("无效的服务端 JSON 地址: {}", e))
    }).anyerr()?;

    let mut expose_local_port = "0.0.0.0:15444".to_string();
    let mut alpn = b"ranyitech.com/port-forwarding/v1".to_vec();

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                i += 1;
            }
            "--expose-port" | "-p" => {
                if i + 1 < args.len() {
                    expose_local_port = args[i + 1].clone();
                    i += 2;
                } else {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "缺少 --expose-port 参数值").into());
                }
            }
            "--alpn" | "-a" => {
                if i + 1 < args.len() {
                    alpn = args[i + 1].as_bytes().to_vec();
                    i += 2;
                } else {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "缺少 --alpn 参数值").into());
                }
            }
            _ => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("未知参数选项: {}", args[i])).into());
            }
        }
    }

    Ok(ClientArgs {
        server_addr,
        expose_local_port,
        alpn,
    })
}

/// 生产级：隧道会话管理器
struct TunnelSession {
    endpoint: Endpoint,
    server_addr: EndpointAddr,
    active_conn: RwLock<Option<iroh::endpoint::Connection>>,
    alpn: Vec<u8>,
}

impl TunnelSession {
    async fn get_connection(&self) -> Result<iroh::endpoint::Connection> {
        {
            let read_guard = self.active_conn.read().await;
            if let Some(conn) = &*read_guard {
                return Ok(conn.clone());
            }
        }

        let mut write_guard = self.active_conn.write().await;
        if let Some(conn) = &*write_guard {
            return Ok(conn.clone());
        }

        let conn = self.endpoint.connect(self.server_addr.clone(), &self.alpn).await?;
        *write_guard = Some(conn.clone());
        
        Ok(conn)
    }

    async fn invalidate_connection(&self) {
        let mut write_guard = self.active_conn.write().await;
        *write_guard = None;
        println!("【清理】已强制清空失效的物理连接缓存。");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli_args = parse_client_args()?;

    let relay_cn: RelayUrl = "https://relay-gz.ranyitech.com".parse().anyerr()?;
    let relay_us: RelayUrl = "https://relay-n.nexalink.link".parse().anyerr()?;
    let relay_map = RelayMap::from_iter([relay_cn, relay_us]);

    let secret_key = load_or_create_key("client.key").await?;
    
    // 生产级：15s 活性探测保活传输层参数配置，锁定公网 NAT 路由端口
    let transport_config = QuicTransportConfig::builder()
        .keep_alive_interval(std::time::Duration::from_secs(15))
        .max_idle_timeout(std::time::Duration::from_secs(60).try_into().ok())
        .build();

    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .relay_mode(RelayMode::Custom(relay_map))
        .transport_config(transport_config)
        .bind()
        .await?;

    println!("\n[本机客户端固定身份标识] ---------------------------------");
    println!("My EndpointID: {}", endpoint.id());
    println!("----------------------------------------------------\n");

    let session = Arc::new(TunnelSession {
        endpoint,
        server_addr: cli_args.server_addr,
        active_conn: RwLock::new(None),
        alpn: cli_args.alpn,
    });

    let listener = TcpListener::bind(&cli_args.expose_local_port).await.anyerr()?;
    println!("正在外网本地监听 {}，请访问此地址以调用内网服务...", cli_args.expose_local_port);

    let concurrency_limiter = Arc::new(Semaphore::new(2000));

    loop {
        let (tcp_stream, _peer_addr) = match listener.accept().await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("接受本地 TCP 失败: {:?}", e);
                continue;
            }
        };

        let session_clone = session.clone();
        let limiter_clone = concurrency_limiter.clone();
        
        tokio::spawn(async move {
            if let Err(e) = forward_tcp_over_iroh(tcp_stream, session_clone, limiter_clone).await {
                eprintln!("穿透转发结束 (Client 侧): {:?}", e);
            }
        });
    }
}

async fn forward_tcp_over_iroh(
    tcp_stream: TcpStream, 
    session: Arc<TunnelSession>,
    limiter: Arc<Semaphore>,
) -> std::io::Result<()> {
    let _permit = limiter.acquire_owned().await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    tcp_stream.set_nodelay(true)?;

    let mut retries = 0;
    const MAX_RETRIES: usize = 10;

    let (iroh_send, iroh_recv) = loop {
        retries += 1;

        match session.get_connection().await {
            Ok(conn) => {
                match conn.open_bi().await {
                    Ok(streams) => break streams,
                    Err(e) => {
                        if retries >= MAX_RETRIES {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::ConnectionReset,
                                format!("在重试 {} 次后，物理通道流依然失效: {:?}", MAX_RETRIES, e)
                            ));
                        }
                        eprintln!("【静默重连 {}/{}】打开逻辑通道失败，正在清除失效连接缓存。原因: {:?}", retries, MAX_RETRIES, e);
                        session.invalidate_connection().await;
                    }
                }
            }
            Err(err) => {
                if retries >= MAX_RETRIES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        format!("在重试 {} 次后，重新建立隧道物理连接仍然失败: {:?}", MAX_RETRIES, err)
                    ));
                }
                eprintln!("【静默重连 {}/{}】物理连接建立失败（服务端可能未完全启动），准备重试。原因: {:?}", retries, MAX_RETRIES, err);
            }
        }

        let delay_ms = std::cmp::min(100 * (1 << (retries - 1)), 2000);
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    };

    // 生产级：采用零缓存、即读即送的高性能流式压缩桥接
    bridge_compressed(tcp_stream, iroh_send, iroh_recv).await?;
    Ok(())
}

async fn bridge_compressed(
    local_tcp: TcpStream,
    iroh_send: iroh::endpoint::SendStream,
    iroh_recv: iroh::endpoint::RecvStream,
) -> std::io::Result<()> {
    let (tcp_recv, tcp_send) = local_tcp.into_split();
    
    let forward_task = compress_and_forward(tcp_recv, iroh_send);
    let backward_task = decompress_and_forward(iroh_recv, tcp_send);
    
    tokio::try_join!(forward_task, backward_task)?;
    Ok(())
}

/// 生产级：4字节长度前缀 + 实时 LZ4 压缩，小包零延迟，即读即送
async fn compress_and_forward<R, W>(mut reader: R, mut writer: W) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024]; // 16KB 帧缓冲
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(e),
        };

        let compressed = lz4_flex::compress(&buf[..n]);
        let len = compressed.len() as u32;
        
        // 4 字节长度前缀 + 压缩体
        writer.write_all(&len.to_be_bytes()).await?;
        writer.write_all(&compressed).await?;
    }
    writer.shutdown().await?;
    Ok(())
}

/// 生产级：4字节长度前缀解析 + 实时解压，零缓冲
async fn decompress_and_forward<R, W>(mut reader: R, mut writer: W) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    loop {
        let mut len_buf = [0u8; 4];
        match tokio::io::AsyncReadExt::read_exact(&mut reader, &mut len_buf).await {
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        
        let mut compressed = vec![0u8; len];
        tokio::io::AsyncReadExt::read_exact(&mut reader, &mut compressed).await?;
        
        let decompressed = lz4_flex::decompress(&compressed, 128 * 1024)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writer.write_all(&decompressed).await?;
    }
    writer.shutdown().await?;
    Ok(())
}
