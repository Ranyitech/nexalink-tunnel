use std::env;
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::presets,
};
use n0_error::{Result, StdResultExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ALPN: &[u8] = b"ranyitech.com/vQKmKbeGD7dls9dfKD239";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // 从命令行中读取服务端地址参数
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("错误: 缺少服务端地址参数！");
        eprintln!("用法: cargo run --bin client '<SERVER_JSON_ADDRESS>'");
        std::process::exit(1);
    }

    let addr_json = &args[1];
    // 反序列化地址
    let addr: EndpointAddr = serde_json::from_str(addr_json).anyerr()?;

    println!("正在尝试连接到服务端...");
    connect_side(addr).await?;

    Ok(())
}

async fn connect_side(addr: EndpointAddr) -> Result<()> {
    let endpoint = Endpoint::bind(presets::N0).await?;

    // 发起连接
    let conn = endpoint.connect(addr, ALPN).await?;

    // 打开一个双向 QUIC 流
    let (mut send, mut recv) = conn.open_bi().await.anyerr()?;

    // 写入测试数据
    println!("发送: \"Hello, world!\"");
    send.write_all(b"Hello, world!").await.anyerr()?;

    // 结束发送
    send.finish().anyerr()?;

    // 读取响应（限制最大读取 1000 字节）
    let response = recv.read_to_end(1000).await.anyerr()?;
    println!("接收到响应: \"{}\"", String::from_utf8_lossy(&response));
    
    assert_eq!(&response, b"Hello, world!");

    // 关闭连接和端点
    conn.close(0u32.into(), b"bye!");
    endpoint.close().await;

    Ok(())
}
