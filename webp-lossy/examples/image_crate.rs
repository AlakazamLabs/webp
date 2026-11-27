//! Encodes any picture the `image` crate can read (PNG, JPEG, ...) to WebP.
//!
//! ```sh
//! cargo run --release --example image_crate -- in.png out.webp [quality]
//! ```

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: image_crate IN OUT [QUALITY]");
        std::process::exit(2);
    }
    let picture = image::open(&args[1])?.to_rgba8();
    let quality: f32 = match args.get(3) {
        Some(q) => q.parse()?,
        None => 75.0,
    };
    let encoder = webp_lossy::Encoder::new(webp_lossy::Config::new(quality));
    let webp = encoder.encode_rgba(picture.width(), picture.height(), picture.as_raw())?;
    std::fs::write(&args[2], &webp)?;
    println!("{} bytes", webp.len());
    Ok(())
}
