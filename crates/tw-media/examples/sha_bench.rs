//! `cargo run -p tw-media --example sha_bench -- FILE`: measures hashing speed.
#[tokio::main]
async fn main() {
    let path = std::env::args().nth(1).expect("usage: sha_bench FILE");
    let start = std::time::Instant::now();
    let hash = tw_media::verify::sha256_file(std::path::Path::new(&path), |_, _| {}).await.unwrap();
    let bytes = std::fs::metadata(&path).unwrap().len();
    let secs = start.elapsed().as_secs_f64();
    println!("{hash}  {:.0} MiB/s", bytes as f64 / secs / (1 << 20) as f64);
}
