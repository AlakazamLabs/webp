//! Checks webp-lossy against outside tools, which run only as separate
//! processes (their source is never read):
//!
//! - `corpus OUT`: encodes a corpus of stills and animations into OUT, with
//!   the encoder's own reconstruction of each still (`.yuv`, I420).
//! - `compare DIR`: after `ci/oracles.sh` has decoded the corpus with
//!   libwebp's `dwebp` and `anim_dump`, checks that dwebp's 4:2:0 output
//!   equals the encoder's reconstruction, and that `image-webp` (a library
//!   used as a black box) decodes every file to exactly dwebp's pixels.
//! - `rd IMAGES OUT`: rate-distortion curves (PSNR, SSIM, SSIMULACRA2)
//!   against `cwebp` at matched settings, and BD-rates.
//! - `speed IMAGES`: single-threaded encode times, ours and cwebp's.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use webp_lossy::{AnimationConfig, AnimationEncoder, Config, Encoder, FrameKind, Picture};

mod metrics;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("corpus") => corpus(Path::new(&args[2])),
        Some("compare") => compare(Path::new(&args[2])),
        Some("rd") => rd(Path::new(&args[2]), Path::new(&args[3])),
        Some("speed") => speed(Path::new(&args[2])),
        Some("alpha") => alpha_sizes(Path::new(&args[2]), Path::new(&args[3])),
        Some("anim") => anim_sizes(Path::new(&args[2]), Path::new(&args[3])),
        _ => {
            eprintln!("usage: webp-oracle corpus OUT | compare DIR | rd IMAGES OUT | speed IMAGES");
            2
        }
    };
    std::process::exit(code);
}

/// A deterministic test picture: gradients, edges, texture and noise.
fn synthetic(w: u32, h: u32, seed: u32, alpha: Option<u8>) -> Vec<u8> {
    let mut s = seed.wrapping_mul(2654435761).max(1);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = (s % 24) as i32 - 12;
            let r = ((x * 255) / w.max(1)) as i32 + noise;
            let g = if (x / 7 + y / 5) % 2 == 0 { 40 } else { 210 } + noise / 2;
            let b = (128.0 + 100.0 * ((x as f32 * 0.21).sin() * (y as f32 * 0.13).cos())) as i32;
            let a = match alpha {
                None => 255,
                Some(0) => 0,
                Some(1) => ((x + y) * 255 / (w + h).max(1)) as i32,
                Some(_) => {
                    if (x / 9 + y / 9) % 3 == 0 {
                        0
                    } else {
                        255
                    }
                }
            };
            out.extend_from_slice(&[
                r.clamp(0, 255) as u8,
                g.clamp(0, 255) as u8,
                b.clamp(0, 255) as u8,
                a as u8,
            ]);
        }
    }
    out
}

fn write_i420(path: &Path, y: &[u8], u: &[u8], v: &[u8]) {
    let mut d = Vec::with_capacity(y.len() + u.len() + v.len());
    d.extend_from_slice(y);
    d.extend_from_slice(u);
    d.extend_from_slice(v);
    fs::write(path, d).expect("write");
}

fn corpus(out: &Path) -> i32 {
    fs::create_dir_all(out).expect("mkdir");
    let mut manifest = String::new();
    let mut n = 0;
    let sizes: &[(u32, u32)] = &[
        (1, 1),
        (2, 3),
        (15, 17),
        (16, 16),
        (33, 9),
        (100, 75),
        (257, 130),
        (640, 480),
    ];
    for (i, &(w, h)) in sizes.iter().enumerate() {
        for method in 0..=6u8 {
            for &quality in &[0.0f32, 30.0, 75.0, 95.0, 100.0] {
                if (w * h > 100 * 100) && method % 3 != 1 && quality != 75.0 {
                    continue;
                }
                for &(segments, simple, sharp, sns) in &[(4u8, false, 0u8, 50u8), (1, true, 3, 0), (3, false, 7, 100)] {
                    let rgba = synthetic(w, h, (i as u32) * 31 + method as u32, None);
                    let mut c = Config::new(quality).with_method(method);
                    c.segments = segments;
                    c.simple_filter = simple;
                    c.filter_sharpness = sharp;
                    c.sns_strength = sns;
                    let o = Encoder::new(c)
                        .encode(Picture::Rgba {
                            width: w,
                            height: h,
                            data: &rgba,
                        })
                        .expect("encode");
                    let name = format!(
                        "still_{w}x{h}_m{method}_q{quality}_s{segments}{}{sharp}_n{sns}",
                        if simple { "S" } else { "N" }
                    );
                    fs::write(out.join(format!("{name}.webp")), &o.data).expect("write");
                    let r = &o.reconstruction;
                    write_i420(&out.join(format!("{name}.yuv")), &r.y, &r.u, &r.v);
                    manifest += &format!("{name}\tstill\t{w}\t{h}\t1\t-\n");
                    n += 1;
                }
            }
        }
    }
    // Fixed filter levels across the range.
    for level in [1u8, 8, 20, 39, 40, 63] {
        let (w, h) = (96, 80);
        let rgba = synthetic(w, h, level as u32, None);
        let mut c = Config::new(40.0);
        c.filter_level = Some(level);
        let o = Encoder::new(c)
            .encode(Picture::Rgba {
                width: w,
                height: h,
                data: &rgba,
            })
            .expect("encode");
        let name = format!("still_filter{level}");
        fs::write(out.join(format!("{name}.webp")), &o.data).expect("write");
        let r = &o.reconstruction;
        write_i420(&out.join(format!("{name}.yuv")), &r.y, &r.u, &r.v);
        manifest += &format!("{name}\tstill\t{w}\t{h}\t1\t-\n");
        n += 1;
    }
    // YUV input, and flat pictures.
    for (k, &(w, h)) in [(31u32, 31u32), (64, 64)].iter().enumerate() {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let y: Vec<u8> = (0..w * h).map(|i| ((i * 13) % 256) as u8).collect();
        let u: Vec<u8> = vec![90; (cw * ch) as usize];
        let v: Vec<u8> = (0..cw * ch).map(|i| (i % 256) as u8).collect();
        let o = Encoder::new(Config::new(80.0))
            .encode(Picture::Yuv420 {
                width: w,
                height: h,
                y: &y,
                u: &u,
                v: &v,
            })
            .expect("encode");
        let name = format!("still_yuv{k}");
        fs::write(out.join(format!("{name}.webp")), &o.data).expect("write");
        let r = &o.reconstruction;
        write_i420(&out.join(format!("{name}.yuv")), &r.y, &r.u, &r.v);
        manifest += &format!("{name}\tstill\t{w}\t{h}\t1\t-\n");
        n += 1;
    }
    // Alpha: all transparent, gradient, cut-out; raw and compressed.
    for (k, a) in [0u8, 1, 2].into_iter().enumerate() {
        for &(aq, exact) in &[(100u8, false), (50, true)] {
            let (w, h) = (73, 41);
            let rgba = synthetic(w, h, 99 + k as u32, Some(a));
            let mut c = Config::new(70.0);
            c.alpha_quality = aq;
            c.exact = exact;
            let o = Encoder::new(c)
                .encode(Picture::Rgba {
                    width: w,
                    height: h,
                    data: &rgba,
                })
                .expect("encode");
            let name = format!("alpha_{a}_aq{aq}");
            fs::write(out.join(format!("{name}.webp")), &o.data).expect("write");
            let r = &o.reconstruction;
            write_i420(&out.join(format!("{name}.yuv")), &r.y, &r.u, &r.v);
            fs::write(
                out.join(format!("{name}.alpha")),
                rgba.iter().skip(3).step_by(4).copied().collect::<Vec<u8>>(),
            )
            .expect("write");
            manifest += &format!("{name}\tstill\t{w}\t{h}\t1\t{aq}\n");
            n += 1;
        }
    }
    // Metadata pass-through.
    {
        let (w, h) = (20, 20);
        let rgba = synthetic(w, h, 5, Some(1));
        let mut e = Encoder::new(Config::new(70.0));
        e.set_icc_profile(vec![1; 41]);
        e.set_exif(b"Exif\0\0MM".to_vec());
        e.set_xmp(b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>".to_vec());
        let o = e
            .encode(Picture::Rgba {
                width: w,
                height: h,
                data: &rgba,
            })
            .expect("encode");
        fs::write(out.join("meta.webp"), &o.data).expect("write");
        let r = &o.reconstruction;
        write_i420(&out.join("meta.yuv"), &r.y, &r.u, &r.v);
        manifest += &format!("meta\tstill\t{w}\t{h}\t1\t100\n");
        fs::write(
            out.join("meta.alpha"),
            rgba.iter().skip(3).step_by(4).copied().collect::<Vec<u8>>(),
        )
        .expect("write");
        n += 1;
    }
    // Animations: moving block, alpha, lossless and mixed frames.
    for (k, kind) in [
        FrameKind::Lossy,
        FrameKind::Lossy,
        FrameKind::Lossless,
        FrameKind::Smaller,
    ]
    .into_iter()
    .enumerate()
    {
        let (w, h) = (90u32, 60u32);
        let mut anim = AnimationEncoder::new(w, h, Config::new(75.0), {
            let mut a = AnimationConfig::default();
            a.frames = kind;
            a.loop_count = k as u16;
            a
        })
        .expect("valid");
        let base = synthetic(w, h, 7, if k == 1 { Some(2) } else { None });
        let mut frames = 0;
        for f in 0..8u32 {
            let mut img = base.clone();
            for y in 10 + f..30 + f {
                for x in 5 + 3 * f..25 + 3 * f {
                    let i = ((y * w + x) * 4) as usize;
                    img[i..i + 4].copy_from_slice(&[255, (f * 30) as u8, 0, 255]);
                }
            }
            if f == 3 {
                // A repeat of frame 2: merged.
                anim.add_frame(&img_prev(&base, w, f - 1), f * 80 + 1).expect("frame");
            }
            anim.add_frame(&img, f * 80 + 40).expect("frame");
            frames += 1;
        }
        let file = anim.finish().expect("finish");
        let name = format!("anim_{k}");
        fs::write(out.join(format!("{name}.webp")), &file).expect("write");
        manifest += &format!("{name}\tanim\t{w}\t{h}\t{frames}\t-\n");
        n += 1;
    }
    fs::write(out.join("manifest.tsv"), manifest).expect("write");
    println!("corpus: {n} files");
    0
}

/// Frame `f` of the moving-block animation (for the repeated frame).
fn img_prev(base: &[u8], w: u32, f: u32) -> Vec<u8> {
    let mut img = base.to_vec();
    for y in 10 + f..30 + f {
        for x in 5 + 3 * f..25 + 3 * f {
            let i = ((y * w + x) * 4) as usize;
            img[i..i + 4].copy_from_slice(&[255, (f * 30) as u8, 0, 255]);
        }
    }
    img
}

/// Reads a binary PAM or PPM: (width, height, channels, pixels).
fn read_pnm(path: &Path) -> Result<(u32, u32, usize, Vec<u8>), String> {
    let d = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.starts_with(b"P7") {
        let end = d.windows(7).position(|w| w == b"ENDHDR\n").ok_or("no ENDHDR")? + 7;
        let head = String::from_utf8_lossy(&d[..end]);
        let field = |k: &str| -> Result<u32, String> {
            head.lines()
                .find_map(|l| l.strip_prefix(k).map(|v| v.trim().parse::<u32>()))
                .ok_or(format!("no {k}"))?
                .map_err(|e| e.to_string())
        };
        let (w, h, depth) = (field("WIDTH")?, field("HEIGHT")?, field("DEPTH")? as usize);
        return Ok((w, h, depth, d[end..].to_vec()));
    }
    if d.starts_with(b"P6") {
        // P6\nW H\n255\n
        let mut fields = Vec::new();
        let mut pos = 2;
        while fields.len() < 3 {
            while d[pos].is_ascii_whitespace() {
                pos += 1;
            }
            let start = pos;
            while !d[pos].is_ascii_whitespace() {
                pos += 1;
            }
            fields.push(
                String::from_utf8_lossy(&d[start..pos])
                    .parse::<u32>()
                    .map_err(|e| e.to_string())?,
            );
        }
        pos += 1;
        return Ok((fields[0], fields[1], 3, d[pos..].to_vec()));
    }
    Err(format!("{}: not PAM or PPM", path.display()))
}

fn to_rgba(channels: usize, px: &[u8]) -> Vec<u8> {
    if channels == 4 {
        return px.to_vec();
    }
    px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
}

fn image_webp_still(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut d = image_webp::WebPDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
    let (w, h) = d.dimensions();
    let mut buf = vec![0u8; d.output_buffer_size().ok_or("size")?];
    d.read_image(&mut buf).map_err(|e| e.to_string())?;
    let ch = if d.has_alpha() { 4 } else { 3 };
    Ok((w, h, to_rgba(ch, &buf)))
}

fn image_webp_frames(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut d = image_webp::WebPDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
    let n = d.num_frames();
    let ch = if d.has_alpha() { 4 } else { 3 };
    let mut out = Vec::new();
    for _ in 0..n {
        let mut buf = vec![0u8; d.output_buffer_size().ok_or("size")?];
        d.read_frame(&mut buf).map_err(|e| e.to_string())?;
        out.push(to_rgba(ch, &buf));
    }
    Ok(out)
}

fn diff_stats(a: &[u8], b: &[u8]) -> (usize, u8) {
    let mut n = 0;
    let mut worst = 0u8;
    for (x, y) in a.iter().zip(b) {
        if x != y {
            n += 1;
            worst = worst.max(x.abs_diff(*y));
        }
    }
    (n + a.len().abs_diff(b.len()), worst)
}

/// As `diff_stats` for RGBA, but the colour of a pixel transparent in both
/// is not compared (it is invisible).
fn diff_visible(a: &[u8], b: &[u8]) -> (usize, u8) {
    let mut n = a.len().abs_diff(b.len());
    let mut worst = 0u8;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let k = if p[3] == 0 && q[3] == 0 { 3 } else { 0 };
        for c in k..4 {
            if p[c] != q[c] {
                n += 1;
                worst = worst.max(p[c].abs_diff(q[c]));
            }
        }
    }
    (n, worst)
}

fn compare(dir: &Path) -> i32 {
    let manifest = fs::read_to_string(dir.join("manifest.tsv")).expect("manifest");
    let mut failed = 0;
    let mut checks = 0;
    for line in manifest.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (name, kind, w, h) = (
            f[0],
            f[1],
            f[2].parse::<u32>().expect("w"),
            f[3].parse::<u32>().expect("h"),
        );
        let data = fs::read(dir.join(format!("{name}.webp"))).expect("webp");
        let mut problems = Vec::new();
        if kind == "still" {
            // 1. dwebp's 4:2:0 output is our reconstruction.
            match fs::read(dir.join(format!("{name}.dwebp.yuv"))) {
                Ok(theirs) => {
                    let ours = fs::read(dir.join(format!("{name}.yuv"))).expect("yuv");
                    let pixels = (w * h) as usize;
                    // With alpha, dwebp appends the alpha plane.
                    let (yuv, alpha) = if theirs.len() == ours.len() + pixels {
                        theirs.split_at(ours.len())
                    } else {
                        (&theirs[..], &[][..])
                    };
                    let (n, worst) = diff_stats(&ours, yuv);
                    if n != 0 {
                        problems.push(format!(
                            "dwebp yuv differs from our reconstruction: {n} samples, worst {worst}"
                        ));
                    }
                    let aq: Option<u32> = f.get(5).and_then(|v| v.parse().ok());
                    match (aq, alpha.is_empty()) {
                        (None, false) => problems.push("dwebp found alpha in an opaque picture".into()),
                        (Some(_), true) => problems.push("dwebp found no alpha".into()),
                        (Some(aq), false) => {
                            let src = fs::read(dir.join(format!("{name}.alpha"))).expect("alpha");
                            // Level reduction: within half a level step.
                            let levels = 2 + aq * 254 / 100;
                            let tol = if levels >= 256 {
                                0
                            } else {
                                255u32.div_ceil(2 * (levels - 1))
                            } as u8;
                            let worst = src.iter().zip(alpha).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
                            if worst > tol {
                                problems.push(format!("alpha differs from the source by {worst} (allowed {tol})"));
                            }
                        }
                        (None, true) => {}
                    }
                }
                Err(e) => problems.push(format!("no dwebp yuv: {e}")),
            }
            // 2. image-webp's RGBA equals dwebp's.
            match (
                read_pnm(&dir.join(format!("{name}.dwebp.pam"))),
                image_webp_still(&data),
            ) {
                (Ok((dw, dh, ch, px)), Ok((iw, ih, ipx))) => {
                    if (dw, dh) != (w, h) || (iw, ih) != (w, h) {
                        problems.push(format!("sizes: dwebp {dw}x{dh}, image-webp {iw}x{ih}, want {w}x{h}"));
                    }
                    let (n, worst) = diff_stats(&to_rgba(ch, &px), &ipx);
                    if n != 0 {
                        problems.push(format!("image-webp differs from dwebp: {n} samples, worst {worst}"));
                    }
                }
                (Err(e), _) => problems.push(format!("dwebp: {e}")),
                (_, Err(e)) => problems.push(format!("image-webp: {e}")),
            }
        } else {
            let frames: usize = f[4].parse().expect("frames");
            match image_webp_frames(&data) {
                Ok(ours) => {
                    if ours.len() != frames {
                        problems.push(format!("image-webp: {} frames, want {frames}", ours.len()));
                    }
                    for (i, fr) in ours.iter().enumerate() {
                        let p = dir.join(format!("{name}.anim")).join(format!("dump_{i:04}.pam"));
                        match read_pnm(&p) {
                            Ok((_, _, ch, px)) => {
                                let (n, worst) = diff_visible(&to_rgba(ch, &px), fr);
                                if n != 0 {
                                    problems.push(format!(
                                        "frame {i}: image-webp differs from anim_dump: {n} samples, worst {worst}"
                                    ));
                                }
                            }
                            Err(e) => problems.push(format!("frame {i}: anim_dump: {e}")),
                        }
                    }
                }
                Err(e) => problems.push(format!("image-webp: {e}")),
            }
        }
        checks += 1;
        if problems.is_empty() {
            println!("PASS {name}");
        } else {
            failed += 1;
            println!("FAIL {name}: {}", problems.join("; "));
        }
    }
    println!("compare: {checks} files, {} passed, {failed} failed", checks - failed);
    (failed > 0) as i32
}

fn read_png_rgb(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(f);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf,
        png::ColorType::Rgba => buf.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0]]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG after EXPAND".into()),
    };
    Ok((w, h, rgb))
}

fn write_ppm(path: &Path, w: u32, h: u32, rgb: &[u8]) {
    let mut d = format!("P6\n{w} {h}\n255\n").into_bytes();
    d.extend_from_slice(rgb);
    fs::write(path, d).expect("write ppm");
}

fn tool(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must name the tool"))
}

fn run(cmd: &mut Command) {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "{cmd:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn decode_dwebp(webp: &Path, ppm: &Path) -> Vec<u8> {
    run(Command::new(tool("DWEBP")).arg(webp).arg("-ppm").arg("-o").arg(ppm));
    let (_, _, ch, px) = read_pnm(ppm).expect("dwebp ppm");
    assert_eq!(ch, 3);
    px
}

/// One encode's measurements.
#[derive(Clone, Copy, Debug)]
struct Point {
    bpp: f64,
    psnr: f64,
    ssim_db: f64,
    ssimu2: f64,
}

fn images_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .expect("images dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    v.sort();
    v
}

fn env_list(name: &str, default: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_else(|_| default.into())
        .split(',')
        .map(str::to_string)
        .collect()
}

fn rd(images: &Path, out: &Path) -> i32 {
    fs::create_dir_all(out).expect("mkdir");
    let qualities: Vec<f32> = env_list("QUALITIES", "20,35,50,65,75,85,92")
        .iter()
        .map(|q| q.parse().expect("q"))
        .collect();
    let method: u8 = std::env::var("METHOD").ok().map_or(4, |m| m.parse().expect("method"));
    let cwebp_args = env_list("CWEBP_ARGS", "-m,4");
    let jobs: usize = std::env::var("JOBS").ok().map_or(2, |j| j.parse().expect("jobs"));
    let files = images_in(images);
    let sns: Option<u8> = std::env::var("SNS").ok().map(|s| s.parse().expect("sns"));
    println!(
        "rd: {} images, qualities {qualities:?}, ours method {method}, cwebp {cwebp_args:?}",
        files.len()
    );
    let results = std::sync::Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(path) = files.get(i) else { break };
                let stem = path.file_stem().expect("stem").to_string_lossy().into_owned();
                let (w, h, rgb) = read_png_rgb(path).expect("png");
                let mut ours = Vec::new();
                let mut theirs = Vec::new();
                for &q in &qualities {
                    let mut c = Config::new(q).with_method(method);
                    if let Some(s) = sns {
                        c.sns_strength = s;
                    }
                    let data = Encoder::new(c).encode_rgb(w, h, &rgb).expect("encode");
                    let f = out.join(format!("{stem}_ours_{q}.webp"));
                    fs::write(&f, &data).expect("write");
                    let dec = decode_dwebp(&f, &out.join(format!("{stem}_ours_{q}.ppm")));
                    ours.push(measure(&rgb, &dec, w, h, data.len()));
                    let g = out.join(format!("{stem}_cwebp_{q}.webp"));
                    run(Command::new(tool("CWEBP"))
                        .arg("-quiet")
                        .arg("-q")
                        .arg(q.to_string())
                        .args(&cwebp_args)
                        .arg(path)
                        .arg("-o")
                        .arg(&g));
                    let size = fs::metadata(&g).expect("cwebp out").len() as usize;
                    let dec = decode_dwebp(&g, &out.join(format!("{stem}_cwebp_{q}.ppm")));
                    theirs.push(measure(&rgb, &dec, w, h, size));
                }
                for p in fs::read_dir(out).expect("dir").filter_map(|e| e.ok()) {
                    if p.path().extension().is_some_and(|x| x == "ppm" || x == "webp")
                        && p.file_name().to_string_lossy().starts_with(&stem)
                    {
                        let _ = fs::remove_file(p.path());
                    }
                }
                results.lock().expect("lock").push((stem, ours, theirs));
            });
        }
    });
    let mut results = results.into_inner().expect("lock");
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let mut tsv = String::from("image\tencoder\tquality\tbpp\tpsnr\tssim_db\tssimulacra2\n");
    let mut bd = [Vec::new(), Vec::new(), Vec::new()];
    for (stem, ours, theirs) in &results {
        for (enc, pts) in [("ours", ours), ("cwebp", theirs)] {
            for (q, p) in qualities.iter().zip(pts.iter()) {
                tsv += &format!(
                    "{stem}\t{enc}\t{q}\t{:.4}\t{:.3}\t{:.3}\t{:.3}\n",
                    p.bpp, p.psnr, p.ssim_db, p.ssimu2
                );
            }
        }
        let rate = |v: &[Point]| v.iter().map(|p| p.bpp).collect::<Vec<_>>();
        let r = [
            metrics::bd_rate(
                &rate(theirs),
                &theirs.iter().map(|p| p.psnr).collect::<Vec<_>>(),
                &rate(ours),
                &ours.iter().map(|p| p.psnr).collect::<Vec<_>>(),
            ),
            metrics::bd_rate(
                &rate(theirs),
                &theirs.iter().map(|p| p.ssim_db).collect::<Vec<_>>(),
                &rate(ours),
                &ours.iter().map(|p| p.ssim_db).collect::<Vec<_>>(),
            ),
            metrics::bd_rate(
                &rate(theirs),
                &theirs.iter().map(|p| p.ssimu2).collect::<Vec<_>>(),
                &rate(ours),
                &ours.iter().map(|p| p.ssimu2).collect::<Vec<_>>(),
            ),
        ];
        println!(
            "{stem}: BD-rate vs cwebp: PSNR {}, SSIM {}, SSIMULACRA2 {}",
            fmt_bd(r[0]),
            fmt_bd(r[1]),
            fmt_bd(r[2])
        );
        for k in 0..3 {
            if let Some(v) = r[k] {
                bd[k].push(v);
            }
        }
    }
    fs::write(out.join("rd.tsv"), tsv).expect("write");
    let mean = |v: &[f64]| {
        if v.is_empty() {
            f64::NAN
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    println!(
        "SUMMARY BD-rate of ours vs cwebp {:?} over {} images (negative = ours smaller): PSNR {:+.2}% ({} images), SSIM {:+.2}% ({}), SSIMULACRA2 {:+.2}% ({})",
        cwebp_args,
        results.len(),
        mean(&bd[0]),
        bd[0].len(),
        mean(&bd[1]),
        bd[1].len(),
        mean(&bd[2]),
        bd[2].len()
    );
    0
}

fn fmt_bd(v: Option<f64>) -> String {
    v.map_or("n/a (no overlap)".into(), |v| format!("{v:+.2}%"))
}

fn measure(src: &[u8], dec: &[u8], w: u32, h: u32, bytes: usize) -> Point {
    let psnr = metrics::psnr_rgb(src, dec);
    let ssim = metrics::ssim_luma(src, dec, w as usize, h as usize);
    let ssimu2 = metrics::ssimulacra2(src, dec, w as usize, h as usize);
    Point {
        bpp: bytes as f64 * 8.0 / (w as f64 * h as f64),
        psnr,
        ssim_db: -10.0 * (1.0 - ssim).max(1e-10).log10(),
        ssimu2,
    }
}

fn speed(images: &Path) -> i32 {
    let files = images_in(images);
    let reps: usize = std::env::var("REPS").ok().map_or(3, |r| r.parse().expect("reps"));
    let tmp = std::env::temp_dir().join("webp-oracle-speed");
    fs::create_dir_all(&tmp).expect("mkdir");
    for method in 0..=6u8 {
        let mut ours = 0f64;
        let mut theirs = 0f64;
        let mut pixels = 0f64;
        for path in &files {
            let (w, h, rgb) = read_png_rgb(path).expect("png");
            let ppm = tmp.join("in.ppm");
            write_ppm(&ppm, w, h, &rgb);
            let mut best_o = f64::MAX;
            let mut best_t = f64::MAX;
            for _ in 0..reps {
                let t = Instant::now();
                let d = Encoder::new(Config::new(75.0).with_method(method))
                    .encode_rgb(w, h, &rgb)
                    .expect("encode");
                best_o = best_o.min(t.elapsed().as_secs_f64());
                std::hint::black_box(d);
                let t = Instant::now();
                run(Command::new(tool("CWEBP"))
                    .arg("-quiet")
                    .arg("-q")
                    .arg("75")
                    .arg("-m")
                    .arg(method.to_string())
                    .arg(&ppm)
                    .arg("-o")
                    .arg(tmp.join("o.webp")));
                best_t = best_t.min(t.elapsed().as_secs_f64());
            }
            ours += best_o;
            theirs += best_t;
            pixels += w as f64 * h as f64;
        }
        println!(
            "SPEED method {method}: ours {:.2} MP/s, cwebp {:.2} MP/s (process wall time incl. reading a PPM), {} images, best of {reps}",
            pixels / 1e6 / ours,
            pixels / 1e6 / theirs,
            files.len()
        );
    }
    0
}

fn write_png_rgba(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    let f = fs::File::create(path).expect("create");
    let mut e = png::Encoder::new(f, w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().expect("header").write_image_data(rgba).expect("png");
}

/// An alpha mask: a soft radial edge, a hard-edged hole and a stripe of
/// partial transparency.
fn mask(w: u32, h: u32) -> Vec<u8> {
    let mut m = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
            let r = (dx * dx + dy * dy).sqrt() / (w.min(h) as f32 / 2.0);
            let mut a = ((1.15 - r) * 4.0).clamp(0.0, 1.0) * 255.0;
            if (x as f32 - w as f32 * 0.3).abs() < w as f32 * 0.08 && (y as f32 - h as f32 * 0.4).abs() < h as f32 * 0.1
            {
                a = 0.0;
            }
            if y % 64 < 8 {
                a *= 0.5;
            }
            m.push(a.round() as u8);
        }
    }
    m
}

fn chunk_len(file: &[u8], fourcc: &[u8; 4]) -> usize {
    let mut pos = 12;
    while pos + 8 <= file.len() {
        let len = u32::from_le_bytes(file[pos + 4..pos + 8].try_into().expect("4")) as usize;
        if &file[pos..pos + 4] == fourcc {
            return len;
        }
        pos += 8 + len + (len & 1);
    }
    0
}

/// Alpha: each test image with a synthetic mask, ours (with the vp8l
/// feature: compressed alpha) against cwebp, both at quality 75 and alpha
/// quality 100; ALPH chunk sizes and the alpha error after dwebp.
fn alpha_sizes(images: &Path, out: &Path) -> i32 {
    fs::create_dir_all(out).expect("mkdir");
    let (mut ours_total, mut theirs_total, mut raw_total) = (0usize, 0usize, 0usize);
    let mut worst = 0u8;
    for path in images_in(images) {
        let stem = path.file_stem().expect("stem").to_string_lossy().into_owned();
        let (w, h, rgb) = read_png_rgb(&path).expect("png");
        let m = mask(w, h);
        let rgba: Vec<u8> = rgb
            .chunks_exact(3)
            .zip(&m)
            .flat_map(|(p, &a)| [p[0], p[1], p[2], a])
            .collect();
        let png_path = out.join(format!("{stem}.png"));
        write_png_rgba(&png_path, w, h, &rgba);
        let ours = Encoder::new(Config::new(75.0))
            .encode_rgba(w, h, &rgba)
            .expect("encode");
        let ours_path = out.join(format!("{stem}_ours.webp"));
        fs::write(&ours_path, &ours).expect("write");
        let theirs_path = out.join(format!("{stem}_cwebp.webp"));
        run(Command::new(tool("CWEBP"))
            .args(["-quiet", "-q", "75", "-m", "4", "-alpha_q", "100"])
            .arg(&png_path)
            .arg("-o")
            .arg(&theirs_path));
        let theirs = fs::read(&theirs_path).expect("cwebp out");
        let pam = out.join("t.pam");
        run(Command::new(tool("DWEBP"))
            .arg(&ours_path)
            .arg("-pam")
            .arg("-o")
            .arg(&pam));
        let (_, _, _, px) = read_pnm(&pam).expect("pam");
        let err = px
            .chunks_exact(4)
            .zip(&m)
            .map(|(p, &a)| p[3].abs_diff(a))
            .max()
            .unwrap_or(0);
        worst = worst.max(err);
        let (a, b) = (chunk_len(&ours, b"ALPH"), chunk_len(&theirs, b"ALPH"));
        println!(
            "{stem}: ALPH ours {a} B, cwebp {b} B, raw {} B; file ours {} B, cwebp {} B; alpha error {err}",
            w * h + 1,
            ours.len(),
            theirs.len()
        );
        ours_total += a;
        theirs_total += b;
        raw_total += (w * h) as usize + 1;
    }
    println!(
        "ALPHA SUMMARY: ALPH bytes ours {ours_total}, cwebp {theirs_total} ({:+.1}%), uncompressed {raw_total}; worst alpha error after dwebp {worst}",
        (ours_total as f64 / theirs_total as f64 - 1.0) * 100.0
    );
    (worst != 0) as i32
}

/// Animation: 24 frames of a 320x240 window panning across each test image
/// with a moving block, at 80 ms; ours (quality 75, method 4) against
/// img2webp -lossy at the same settings, both decoded by anim_dump; sizes
/// and the mean PSNR over frames.
fn anim_sizes(images: &Path, out: &Path) -> i32 {
    fs::create_dir_all(out).expect("mkdir");
    let img2webp = Path::new(&tool("CWEBP")).with_file_name("img2webp");
    let anim_dump = Path::new(&tool("CWEBP")).with_file_name("anim_dump");
    let (fw, fh) = (320u32, 240u32);
    let (mut ours_b, mut theirs_b, mut ours_p, mut theirs_p, mut n) = (0usize, 0usize, 0f64, 0f64, 0usize);
    let mut bds = Vec::new();
    for path in images_in(images).iter().take(8) {
        let stem = path.file_stem().expect("stem").to_string_lossy().into_owned();
        let (w, _h, rgb) = read_png_rgb(path).expect("png");
        let mut frames = Vec::new();
        for f in 0..24u32 {
            let mut fr = Vec::with_capacity((fw * fh * 4) as usize);
            let ox = (f * 4).min(w - fw);
            for y in 0..fh {
                for x in 0..fw {
                    let i = ((y * w + x + ox) * 3) as usize;
                    let inside = (x as i32 - (40 + f as i32 * 9)).abs() < 20 && (y as i32 - 120).abs() < 20;
                    if inside {
                        fr.extend_from_slice(&[230, 40, 40, 255]);
                    } else {
                        fr.extend_from_slice(&[rgb[i], rgb[i + 1], rgb[i + 2], 255]);
                    }
                }
            }
            frames.push(fr);
        }
        for (i, fr) in frames.iter().enumerate() {
            write_png_rgba(&out.join(format!("f{i:02}.png")), fw, fh, fr);
        }
        // (bytes, mean PSNR) per quality, ours then img2webp's.
        let mut curves: [Vec<(f64, f64)>; 2] = [Vec::new(), Vec::new()];
        for q in [35.0f32, 50.0, 65.0, 75.0, 85.0] {
            let mut enc = AnimationEncoder::new(fw, fh, Config::new(q), AnimationConfig::default()).expect("valid");
            let qs = q.to_string();
            let mut args: Vec<std::ffi::OsString> = vec![
                "-loop".into(),
                "0".into(),
                "-lossy".into(),
                "-q".into(),
                qs.into(),
                "-m".into(),
                "4".into(),
            ];
            for (i, fr) in frames.iter().enumerate() {
                enc.add_frame(fr, i as u32 * 80).expect("frame");
                args.extend([
                    "-d".into(),
                    "80".into(),
                    out.join(format!("f{i:02}.png")).into_os_string(),
                ]);
            }
            let ours_path = out.join(format!("{stem}_ours.webp"));
            fs::write(&ours_path, enc.finish().expect("finish")).expect("write");
            let theirs_path = out.join(format!("{stem}_img2webp.webp"));
            run(Command::new(&img2webp).args(&args).arg("-o").arg(&theirs_path));
            for (k, file) in [&ours_path, &theirs_path].into_iter().enumerate() {
                let d = out.join(format!("dump{k}"));
                let _ = fs::remove_dir_all(&d);
                fs::create_dir_all(&d).expect("mkdir");
                run(Command::new(&anim_dump).arg("-folder").arg(&d).arg("-pam").arg(file));
                let mut psnr = 0f64;
                for (i, fr) in frames.iter().enumerate() {
                    let (_, _, ch, px) = read_pnm(&d.join(format!("dump_{i:04}.pam"))).expect("frame");
                    let px = to_rgba(ch, &px);
                    let a: Vec<u8> = fr.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
                    let b: Vec<u8> = px.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
                    psnr += metrics::psnr_rgb(&a, &b) / frames.len() as f64;
                }
                curves[k].push((fs::metadata(file).expect("file").len() as f64, psnr));
            }
        }
        let rate = |c: &[(f64, f64)]| c.iter().map(|p| p.0).collect::<Vec<_>>();
        let dist = |c: &[(f64, f64)]| c.iter().map(|p| p.1).collect::<Vec<_>>();
        let bd = metrics::bd_rate(
            &rate(&curves[1]),
            &dist(&curves[1]),
            &rate(&curves[0]),
            &dist(&curves[0]),
        );
        let at75 = (curves[0][3], curves[1][3]);
        println!(
            "{stem}: BD-rate (PSNR) vs img2webp -lossy {}; at quality 75 ours {} B {:.2} dB, img2webp {} B {:.2} dB",
            fmt_bd(bd),
            at75.0 .0,
            at75.0 .1,
            at75.1 .0,
            at75.1 .1
        );
        if let Some(b) = bd {
            bds.push(b);
        }
        ours_b += at75.0 .0 as usize;
        theirs_b += at75.1 .0 as usize;
        ours_p += at75.0 .1;
        theirs_p += at75.1 .1;
        n += 1;
    }
    let mean_bd = bds.iter().sum::<f64>() / bds.len().max(1) as f64;
    println!(
        "ANIM BD-rate (PSNR) of ours vs img2webp -lossy -m 4 over {} clips: {mean_bd:+.2}%",
        bds.len()
    );
    println!(
        "ANIM SUMMARY at quality 75 over {n} clips: ours {ours_b} B at {:.2} dB, img2webp -lossy {theirs_b} B at {:.2} dB ({:+.1}% bytes)",
        ours_p / n as f64,
        theirs_p / n as f64,
        (ours_b as f64 / theirs_b as f64 - 1.0) * 100.0
    );
    0
}
