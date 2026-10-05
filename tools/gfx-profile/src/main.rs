//! Serial, GPU-completion-bounded diagnostic. Not a presentation/FPS benchmark.
use fpl_gfx::{
    AnalogClock, Color, MaterialTheme, Rect, RenderContext, RenderOptions, Renderer,
    RendererConfig, Scene, Theme, Viewport,
};
use std::time::Instant;

fn percentiles(values: &mut [f64]) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    serde_json::json!({"n":n,"p50_ms":values[n/2],"p95_ms":values[(n*95/100).min(n-1)],"p99_ms":values[(n*99/100).min(n-1)]})
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    pollster::block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let width = std::env::args()
        .nth(1)
        .map(|s| s.parse::<u32>())
        .transpose()?
        .unwrap_or(1920);
    let height = std::env::args()
        .nth(2)
        .map(|s| s.parse::<u32>())
        .transpose()?
        .unwrap_or(1080);
    let sample_count = std::env::args()
        .nth(3)
        .map(|s| s.parse::<u32>())
        .transpose()?
        .unwrap_or(1);
    if !matches!(sample_count, 1 | 4) {
        return Err("sample count must be 1 or 4".into());
    }
    if !(64..=4096).contains(&width) || !(64..=4096).contains(&height) {
        return Err("dimensions must be 64..4096".into());
    }
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await?;
    let info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits {
                max_color_attachments: 1,
                ..wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits())
            },
            ..Default::default()
        })
        .await?;
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gfx profile target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut config = RendererConfig::new(format);
    config.sample_count = sample_count;
    let mut renderer = Renderer::new(&device, config);
    let mut theme = Theme::studio_dark();
    theme.materials = MaterialTheme {
        gloss: 0.0,
        grain: 0.0,
        emission: 0.0,
    };
    let mut clock = Scene::with_capacity(65);
    AnalogClock {
        hour: 10,
        minute: 9,
        second: 30.5,
    }
    .append(&mut clock, Rect::new(0.0, 0.0, width as f32, height as f32));
    let mut output = Vec::new();
    for (name, range) in [
        ("clear", 0..0),
        ("face", 0..1),
        ("ticks", 1..61),
        ("hands", 61..65),
        ("clock", 0..65),
    ] {
        let mut scene = Scene::with_capacity(range.len());
        for primitive in clock.iter().skip(range.start).take(range.len()) {
            scene.push(*primitive);
        }
        let mut completion = Vec::new();
        let mut cpu = Vec::new();
        for sample in 0..30 {
            let start = Instant::now();
            let mut encoder = device.create_command_encoder(&Default::default());
            renderer.render(
                RenderContext {
                    device: &device,
                    queue: &queue,
                    encoder: &mut encoder,
                    target: &view,
                },
                Viewport::new(width as f32, height as f32),
                &theme,
                &scene,
                RenderOptions {
                    clear: Some(Color::linear(0.0, 0.0, 0.0, 1.0)),
                },
            )?;
            let submit_cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
            queue.submit([encoder.finish()]);
            device.poll(wgpu::PollType::wait_indefinitely())?;
            if sample >= 5 {
                completion.push(start.elapsed().as_secs_f64() * 1000.0);
                cpu.push(submit_cpu_ms);
            }
        }
        output.push(serde_json::json!({"scenario":name,"primitives":scene.len(),"cpu_encode":percentiles(&mut cpu),"submit_to_completion":percentiles(&mut completion)}));
    }
    println!(
        "{}",
        serde_json::json!({"adapter":info.name,"driver":info.driver,"device_type":format!("{:?}",info.device_type),"width":width,"height":height,"sample_count":sample_count,"format":"Bgra8UnormSrgb","warmup":5,"samples":25,"results":output,"limitations":"Serial offscreen completion latency includes CPU, submission and GPU execution, not presentation or timestamp-query GPU time. No readback or MSAA resolve in measured loop."})
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percentile_selection_is_explicit() {
        let mut values = vec![3.0, 1.0, 2.0];
        let s = percentiles(&mut values);
        assert_eq!(s["p50_ms"], 2.0);
        assert_eq!(s["p95_ms"], 3.0);
    }
}
