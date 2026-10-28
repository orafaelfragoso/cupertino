//! Lowering of a composed [`SceneFrame`] to the elements of the GLES damage tracker: one element per item,
//! front to back, with the cache slots and commit counters the renderer has always used.

use cupertino_scene::{RoundedRect, SceneFrame, Shadow, Source};

use super::compose::logical_rect;
use super::programs::{u4, Programs};
use super::shader_rect::{ElementCache, ElementKey, ShaderRect, Sig};
use super::ui::{rounded_rect, RoundedRectSpec};
use super::CupertinoElement;

/// What lowering draws with: shader programs, the element cache of the output and its scale.
pub struct Lowering<'a> {
    pub programs: &'a Programs,
    pub cache: &'a mut ElementCache,
    pub scale: f64,
}

/// The elements of `frame` front to back; `locals` holds what its [`Source::LocalPass`] items refer to.
pub fn lower(
    frame: &SceneFrame,
    mut locals: Vec<Option<CupertinoElement>>,
    ctx: &mut Lowering<'_>,
) -> Vec<CupertinoElement> {
    let mut elements = Vec::with_capacity(frame.items().len());
    for (_, item) in frame.items() {
        let key = ElementKey::new(item.id.kind.key(), item.id.index);
        match &item.source {
            Source::LocalPass { index } => match locals.get_mut(*index).and_then(Option::take) {
                Some(element) => elements.push(element),
                None => {
                    debug_assert!(false, "local pass {index} is missing or drawn twice");
                    tracing::error!(index, "scene local pass missing, item skipped");
                }
            },
            Source::RoundedRect(rect) => {
                elements.push(rounded(ctx, key, rect, item.opacity as f32).into());
            }
            Source::Shadow(shadow) => {
                elements.push(shadow_rect(ctx, key, shadow, item.opacity).into());
            }
            Source::Wallpaper { rect, colours } => {
                elements.push(wallpaper(ctx, key, logical_rect(*rect), colours).into());
            }
            other => {
                debug_assert!(false, "composition does not emit {other:?}");
                tracing::error!(source = ?other, "scene source not lowerable, item skipped");
            }
        }
    }
    elements
}

fn rounded(ctx: &mut Lowering<'_>, key: ElementKey, rect: &RoundedRect, alpha: f32) -> ShaderRect {
    let spec = RoundedRectSpec {
        rect: logical_rect(rect.rect),
        radius: rect.radius,
        continuous: !rect.style.is_circular(),
        fill: rect.fill,
        border: rect.border,
        alpha,
    };
    rounded_rect(ctx.programs, ctx.cache, ctx.scale, key, &spec)
}

pub fn shadow_rect(
    ctx: &mut Lowering<'_>,
    key: ElementKey,
    shadow: &Shadow,
    alpha: f64,
) -> ShaderRect {
    let area = logical_rect(shadow.area);
    let caster = [
        (shadow.caster.0.x() - shadow.area.0.x()) as f32,
        (shadow.caster.0.y() - shadow.area.0.y()) as f32,
        shadow.caster.0.width() as f32,
        shadow.caster.0.height() as f32,
    ];
    let extent = shadow.radius * shadow.style.expansion();
    let uniforms = vec![
        u4("caster", caster),
        u4(
            "params",
            [
                shadow.radius as f32,
                shadow.sigma as f32,
                shadow.density as f32,
                shadow.offset_y as f32,
            ],
        ),
        u4(
            "flags",
            [
                shadow.cutout as u8 as f32,
                shadow.style.exponent() as f32,
                ctx.scale as f32,
                extent as f32,
            ],
        ),
    ];
    let sig = Sig::new()
        .rect(area)
        .f(shadow.radius)
        .f(shadow.sigma)
        .f(shadow.density)
        .f(shadow.offset_y)
        .f(alpha)
        .f(ctx.scale)
        .u(shadow.cutout as u64)
        .finish();
    let (id, commit) = ctx.cache.slot(key, sig);
    ShaderRect::new(
        ctx.programs.shadow.clone(),
        id,
        commit,
        area,
        alpha as f32,
        uniforms,
        false,
    )
}

fn wallpaper(
    ctx: &mut Lowering<'_>,
    key: ElementKey,
    area: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
    colours: &[[f32; 4]; 4],
) -> ShaderRect {
    let mut sig = Sig::new().rect(area);
    for v in colours.iter().flatten() {
        sig = sig.f(f64::from(*v));
    }
    let (id, commit) = ctx.cache.slot(key, sig.finish());
    ShaderRect::new(
        ctx.programs.wallpaper.clone(),
        id,
        commit,
        area,
        1.0,
        vec![
            u4("c0", colours[0]),
            u4("c1", colours[1]),
            u4("c2", colours[2]),
            u4("c3", colours[3]),
        ],
        true,
    )
}
