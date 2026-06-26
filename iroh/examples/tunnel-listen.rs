use iroh::{
    Endpoint, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey,
    endpoint::{Connection, presets, QuicTransportConfig},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use n0_error::{Result, StdResultExt};
use std::collections::HashSet;
use std::env;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        Ok(key)
    }
}

/// 解析服务端命令行参数
struct ServerArgs {
    local_service_addr: String,
    alpn: Vec<u8>,
    whitelist: HashSet<EndpointId>,
}

fn parse_server_args() -> Result<ServerArgs> {
    let args: Vec<String> = env::args().collect();
    let mut local_service_addr = "127.0.0.1:11435".to_string();
    let mut alpn = b"ranyitech.com/port-forwarding/v1".to_vec();
    let mut whitelist = HashSet::new();
    
    if let Ok(id) = EndpointId::from_str("c2381bc8c69ce7ca39804b9b3edb04c014170500866272867ccc0a7daae505ad") {
        whitelist.insert(id);
    }

    if args.contains(&"-h".to_string()) || args.contains(&"--help".to_string()) {
        println!("Iroh 内网穿透服务端 (生产高能版)");
        println!("用法: cargo run --bin server -- [options]");
        println!();
        println!("选项:");
        println!("  -l, --local-addr <addr>   需要穿透并暴露的本地服务地址 (默认: 127.0.0.1:11435)");
        println!("  -a, --alpn <alpn_name>    自定义 ALPN 隧道应用协议 (默认: ranyitech.com/port-forwarding/v1)");
        println!("  -w, --whitelist <ids>     允许连接的可信客户端 ID (多个使用逗号分隔，不传入则使用默认内置白名单)");
        std::process::exit(0);
    }

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                i += 1;
            }
            "--local-addr" | "-l" => {
                if i + 1 < args.len() {
                    local_service_addr = args[i + 1].clone();
                    i += 2;
                } else {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "缺少 --local-addr 参数值").into());
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
            "--whitelist" | "-w" => {
                if i + 1 < args.len() {
                    whitelist.clear();
                    for id_str in args[i + 1].split(',') {
                        let id_str = id_str.trim();
                        if !id_str.is_empty() {
                            let id = EndpointId::from_str(id_str).map_err(|e| {
                                std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("无效的客户端 EndpointId: {}", e))
                            }).anyerr()?;
                            whitelist.insert(id);
                        }
                    }
                    i += 2;
                } else {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "缺少 --whitelist 参数值").into());
                }
            }
            _ => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("未知参数选项: {}", args[i])).into());
            }
        }
    }

    Ok(ServerArgs {
        local_service_addr,
        alpn,
        whitelist,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    let cli_args = parse_server_args()?;
    let local_service_addr_clone = cli_args.local_service_addr.clone();

    let secret_key = load_or_create_key("server.key").await?;
    let relay_cn: RelayUrl = "https://relay-gz.ranyitech.com".parse().anyerr()?;
    let relay_us: RelayUrl = "https://relay-n.nexalink.link".parse().anyerr()?;
    let relay_map = RelayMap::from_iter([relay_cn, relay_us]);
    
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
        
    let concurrency_limiter = Arc::new(Semaphore::new(2000));

    let handler = ProxyHandler { 
        concurrency_limiter,
        local_service_addr: cli_args.local_service_addr,
        whitelist: cli_args.whitelist,
    };

    let router = Router::builder(endpoint)
        .accept(&cli_args.alpn, handler)
        .spawn();
        
    router.endpoint().online().await;
    
    let addr = router.endpoint().addr();
    let addr_json = serde_json::to_string(&addr).anyerr()?;
    
    println!("\n==================================================");
    println!("内网穿透服务端已启动！目标暴露内网服务：{}", local_service_addr_clone);
    println!("请将以下永久有效的 JSON 地址复制并传递给外网客户端：");
    println!("{}", addr_json);
    println!("==================================================\n");
    
    tokio::signal::ctrl_c().await.anyerr()?;
    router.shutdown().await.anyerr()?;
    Ok(())
}

#[derive(Debug, Clone)]
struct ProxyHandler {
    concurrency_limiter: Arc<Semaphore>,
    local_service_addr: String,
    whitelist: HashSet<EndpointId>,
}

impl ProtocolHandler for ProxyHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let client_id = connection.remote_id();
        
        if !self.whitelist.is_empty() && !self.whitelist.contains(&client_id) {
            eprintln!("【安全拦截】检测到非授权的外部客户端尝试接入！NodeID: {}", client_id);
            connection.close(1u32.into(), b"unauthorized");
            return Ok(());
        }
        println!("【隧道连接成功】可信任的客户端已接入: {}", client_id);
        
        let local_addr = self.local_service_addr.clone();

        loop {
            let (iroh_send, iroh_recv) = match connection.accept_bi().await {
                Ok(streams) => streams,
                Err(e) => {
                    println!("Iroh 物理隧道连接断开或关闭: {:?}", e);
                    break;
                }
            };

            let permit = match self.concurrency_limiter.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    eprintln!("并发流控制器信号量获取失败: {:?}", e);
                    break;
                }
            };

            let local_addr_clone = local_addr.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) = bridge_streams_to_local(iroh_send, iroh_recv, &local_addr_clone).await {
                    eprintln!("流对拷处理中断: {:?}", e);
                }
            });
        }
        Ok(())
    }
}


async fn bridge_streams_to_local(
    iroh_send: iroh::endpoint::SendStream,
    iroh_recv: iroh::endpoint::RecvStream,
    local_service_addr: &str,
) -> std::io::Result<()> {
    let mut local_retries = 0;
    const MAX_LOCAL_RETRIES: usize = 30;

    let local_tcp = loop {
        local_retries += 1;
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            TcpStream::connect(local_service_addr),
        ).await {
            Ok(Ok(stream)) => break stream,
            Ok(Err(e)) => {
                if local_retries >= MAX_LOCAL_RETRIES {
                    eprintln!("【内网连接失败】已达到最大尝试次数（30秒），本地 AI 服务仍未就绪: {:?}", e);
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            Err(_) => {
                if local_retries >= MAX_LOCAL_RETRIES {
                    eprintln!("【内网连接超时】已达到最大尝试次数（30秒），连接本地 AI 服务超时");
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "连接本地服务超时"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
        }
    };

    local_tcp.set_nodelay(true)?;

    // 生产级：启用极速分帧压缩与纠错桥接转发
    bridge_compressed(local_tcp, iroh_send, iroh_recv).await?;
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
