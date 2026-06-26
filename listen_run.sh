cargo build --release --example tunnel-listen && cargo build --release --example tunnel-send
nohup ./target/release/examples/tunnel-listen -l "127.0.0.1:11435" -w "c2381bc8c69ce7ca39804b9b3edb04c014170500866272867ccc0a7daae505ad" -a "ranyitech.com/port-forwarding/v1" > ./tunnel-listen.log 2>&1 &
