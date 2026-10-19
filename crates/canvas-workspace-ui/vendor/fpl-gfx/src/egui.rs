//! egui composition on caller-owned wgpu targets, without windows or readback.
//! Integration seam follows Synesthesia's egui-wgpu pass (synesthesia-canvas,
//! upstream 4735822); domain drawing and host transport are deliberately absent.

/// Caller-owned render resources and attachment policy for one composition.
pub struct Target<'a> {
    /// Device that created this pass and target.
    pub device: &'a wgpu::Device,
    /// Queue belonging to the same device.
    pub queue: &'a wgpu::Queue,
    /// Encoder retained by the host's render graph.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Current color attachment, matching the format passed to [`Pass::new`].
    pub view: &'a wgpu::TextureView,
    /// Physical target dimensions, before egui's logical pixel scaling.
    pub size: [u32; 2],
    /// Clear for a standalone target, Load when compositing over existing content.
    pub load: wgpu::LoadOp<wgpu::Color>,
}

/// Commands that must be submitted before the host submits its primary encoder.
pub struct Painted {
    /// Paint callbacks may need their own preparation command buffers.
    pub preparation: Vec<wgpu::CommandBuffer>,
    /// Number of clipped primitives submitted to the renderer.
    pub primitives: usize,
}

/// Invalid dimensions or pixel scale, rejected before consuming egui paint data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid egui target dimensions or pixel scale")]
pub struct InvalidTarget;

/// Retained egui GPU resources for one host device and attachment format.
/// The host retains egui's Context and supplies input/platform integration.
pub struct Pass {
    renderer: egui_wgpu::Renderer,
}
impl Pass {
    /// Create a compositor pass; does not create a device, surface or event loop.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: egui_wgpu::Renderer::new(
                device,
                format,
                egui_wgpu::RendererOptions::PREDICTABLE,
            ),
        }
    }

    /// Consume paint data while leaving platform/viewport outputs for the host.
    /// The host must handle clipboard, IME, cursor, accessibility and repaint
    /// deadlines, submit preparation buffers before its encoder, and present.
    ///
    /// # Errors
    /// Returns [`InvalidTarget`] before mutation for invalid target geometry.
    #[allow(clippy::needless_pass_by_value)] // The exclusive encoder borrow ends with this paint call.
    pub fn paint(
        &mut self,
        context: &::egui::Context,
        output: &mut ::egui::FullOutput,
        target: Target<'_>,
    ) -> Result<Painted, InvalidTarget> {
        validate(
            target.size,
            output.pixels_per_point,
            target.device.limits().max_texture_dimension_2d,
        )?;
        let textures = std::mem::take(&mut output.textures_delta);
        for (id, delta) in &textures.set {
            self.renderer
                .update_texture(target.device, target.queue, *id, delta);
        }
        let jobs = context.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: target.size,
            pixels_per_point: output.pixels_per_point,
        };
        let preparation = self.renderer.update_buffers(
            target.device,
            target.queue,
            target.encoder,
            &jobs,
            &screen,
        );
        {
            let pass = target
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Shared egui composition"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: target.load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
        Ok(Painted {
            preparation,
            primitives: jobs.len(),
        })
    }
}

fn validate(size: [u32; 2], scale: f32, max: u32) -> Result<(), InvalidTarget> {
    if size.into_iter().any(|v| v == 0 || v > max) || !scale.is_finite() || scale <= 0.0 {
        Err(InvalidTarget)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unusable_geometry_before_touching_gpu_or_paint_data() {
        assert_eq!(validate([0, 800], 1.0, 4096), Err(InvalidTarget));
        assert_eq!(validate([5000, 800], 1.0, 4096), Err(InvalidTarget));
        assert_eq!(validate([1280, 800], f32::NAN, 4096), Err(InvalidTarget));
        assert_eq!(validate([1280, 800], 0.0, 4096), Err(InvalidTarget));
        assert!(validate([1280, 800], 1.5, 4096).is_ok());
    }
}
