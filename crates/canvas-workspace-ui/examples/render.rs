//! Real offscreen GPU review harness. Readback is for exported QA artifacts only.
use canvas_workspace_ui::shell::{Shell, View};
use coherence_native::{Density, Mode, Tokens};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        return Err(
            "Usage: render OUTPUT.png [WIDTH HEIGHT light|dark|sepia mail|work|calendar]".into(),
        );
    }
    let width = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(1600);
    let height = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(1000);
    if width == 0 || height == 0 || width > 3840 || height > 2160 {
        return Err("Invalid review dimensions".into());
    }
    let mode = match args.get(4).map(String::as_str).unwrap_or("dark") {
        "dark" => Mode::Dark,
        "light" => Mode::Light,
        "sepia" => Mode::Sepia,
        _ => return Err("Unknown appearance".into()),
    };
    pollster::block_on(render(
        &args[1],
        width,
        height,
        mode,
        args.get(5).map(String::as_str).unwrap_or("mail"),
    ))
}

async fn render(
    path: &str,
    width: u32,
    height: u32,
    mode: Mode,
    view: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tokens = Tokens::new(mode, Density::Compact);
    let context = egui::Context::default();
    context.set_style(coherence_native::egui::style(&tokens)?);
    let mut shell = Shell::demo()?;
    shell.mode = mode;
    shell.view = match view {
        "mail" | "commands" | "compose" | "thread" => View::Mail,
        "work" => View::Work,
        "calendar" => View::Calendar,
        _ => return Err("Unknown view".into()),
    };
    shell.mail.palette = view == "commands";
    shell.mail.compose_open = view == "compose";
    shell.mail.reader_open = view == "thread";
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width as f32, height as f32),
        )),
        time: Some(0.0),
        ..Default::default()
    };
    let mut output = context.run(input.clone(), |ctx| shell.show(ctx));
    for frame in 1..=3 {
        let mut settled = input.clone();
        settled.time = Some(frame as f64);
        output.append(context.run(settled, |ctx| shell.show(ctx)));
    }
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Work inbox review"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut pass = fpl_gfx::egui::Pass::new(&device, format);
    let mut encoder = device.create_command_encoder(&Default::default());
    let painted = pass.paint(
        &context,
        &mut output,
        fpl_gfx::egui::Target {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            view: &view,
            size: [width, height],
            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        },
    )?;
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Review readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(painted.preparation.into_iter().chain([encoder.finish()]));
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv_timeout(std::time::Duration::from_secs(10))??;
    let mapped = buffer.slice(..).get_mapped_range();
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in mapped.chunks_exact(stride as usize) {
        pixels.extend_from_slice(&row[..width as usize * 4]);
    }
    if pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[3] != 255)
    {
        return Err("Review frame has unexpected transparency".into());
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    let mut sidecar = std::fs::File::create(format!("{path}.json"))?;
    writeln!(
        sidecar,
        "{}",
        serde_json::json!({"width":width,"height":height,"items":shell.owner.threads.len(),"adapter":adapter.get_info().name,"primitives":painted.primitives})
    )?;
    println!(
        "rendered {width}x{height} with {} fixture threads",
        shell.owner.threads.len()
    );
    Ok(())
}
