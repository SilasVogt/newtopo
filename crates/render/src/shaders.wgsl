// newtopo viewport shaders. One module, several entry points.
// Colours arrive as sRGB + straight alpha and are linearised here; the
// colour target is an sRGB texture, so the hardware re-encodes on write.

struct Globals {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // x = width px, y = height px, z = pixels per point, w = near plane
    viewport: vec4<f32>,
    // x = ortho (0/1), y = overlay bias, z = flat shading (0/1), w = scene radius
    params: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Move a view-space point towards the camera without changing where it
// lands on screen (perspective: scale towards the eye; ortho: shift in z).
fn bias_view(p: vec3<f32>, amount: f32) -> vec3<f32> {
    if (g.params.x > 0.5) {
        return p + vec3<f32>(0.0, 0.0, amount * g.params.w * 4.0);
    }
    return p * (1.0 - amount);
}

// ---------------------------------------------------------------- background

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
};

@vertex
fn vs_bg(@builtin(vertex_index) i: u32) -> BgOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: BgOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    out.t = y;
    return out;
}

@fragment
fn fs_bg(in: BgOut) -> @location(0) vec4<f32> {
    // Slight ease so the gradient sits mostly in the lower half.
    let t = smoothstep(0.0, 1.0, in.t);
    let c = mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
    return vec4<f32>(c, 1.0);
}

// ---------------------------------------------------------------- meshes

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) vpos: vec3<f32>,
    @location(1) vnormal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_mesh(in: MeshIn) -> MeshOut {
    var out: MeshOut;
    let vp = g.view * vec4<f32>(in.pos, 1.0);
    out.vpos = vp.xyz;
    out.vnormal = (g.view * vec4<f32>(in.normal, 0.0)).xyz;
    out.clip = g.proj * vp;
    out.color = in.color;
    return out;
}

fn shade(base_srgb: vec3<f32>, n_in: vec3<f32>, vpos: vec3<f32>) -> vec3<f32> {
    let v = select(normalize(-vpos), vec3<f32>(0.0, 0.0, 1.0), g.params.x > 0.5);
    var n = n_in;
    if (dot(n, v) < 0.0) {
        n = -n; // two-sided: open shells and back faces still read well
    }
    let base = srgb_to_linear(base_srgb);
    let key = normalize(vec3<f32>(-0.45, 0.62, 0.64));
    let fill = normalize(vec3<f32>(0.75, -0.2, 0.45));
    let diff = 0.72 * max(dot(n, key), 0.0) + 0.22 * max(dot(n, fill), 0.0);
    let hemi = n.y * 0.5 + 0.5;
    let amb = mix(vec3<f32>(0.075, 0.08, 0.09), vec3<f32>(0.19, 0.2, 0.22), hemi);
    let h = normalize(key + v);
    let spec = 0.2 * pow(max(dot(n, h), 0.0), 48.0);
    let rim = 0.07 * pow(1.0 - max(dot(n, v), 0.0), 3.0);
    return base * (amb * 1.5 + diff) + vec3<f32>(spec) + rim * vec3<f32>(0.55, 0.65, 0.8);
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    let flat_n = normalize(cross(dpdx(in.vpos), dpdy(in.vpos)));
    var n = normalize(in.vnormal);
    if (g.params.z > 0.5) {
        n = flat_n;
    }
    return vec4<f32>(shade(in.color.rgb, n, in.vpos), 1.0);
}

// Translucent surfaces: biased towards the camera, lightly shaded.
@vertex
fn vs_translucent(in: MeshIn) -> MeshOut {
    var out: MeshOut;
    let vp = bias_view((g.view * vec4<f32>(in.pos, 1.0)).xyz, g.params.y * 0.5);
    out.vpos = vp;
    out.vnormal = (g.view * vec4<f32>(in.normal, 0.0)).xyz;
    out.clip = g.proj * vec4<f32>(vp, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_translucent(in: MeshOut) -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
}

// ---------------------------------------------------------------- lines

struct LineIn {
    @location(0) a: vec3<f32>,
    @location(1) width: f32,
    @location(2) b: vec3<f32>,
    @location(3) dash: f32,
    @location(4) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) across: f32,
    @location(2) along: f32,
    @location(3) @interpolate(flat) hw_dash: vec2<f32>,
};

// Quad corners as (t along the segment, side): two triangles.
fn corner(i: u32) -> vec2<f32> {
    switch (i) {
        case 0u: { return vec2<f32>(0.0, -1.0); }
        case 1u: { return vec2<f32>(1.0, -1.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        case 3u: { return vec2<f32>(0.0, -1.0); }
        case 4u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(0.0, 1.0); }
    }
}

@vertex
fn vs_line(@builtin(vertex_index) vi: u32, in: LineIn) -> LineOut {
    var out: LineOut;
    var va = (g.view * vec4<f32>(in.a, 1.0)).xyz;
    var vb = (g.view * vec4<f32>(in.b, 1.0)).xyz;
    if (g.params.x < 0.5) {
        // Clip against the near plane in view space (camera looks down -z).
        let nz = -g.viewport.w * 1.001;
        if (va.z > nz && vb.z > nz) {
            out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
            return out;
        }
        if (va.z > nz) {
            va = mix(va, vb, (nz - va.z) / (vb.z - va.z));
        } else if (vb.z > nz) {
            vb = mix(vb, va, (nz - vb.z) / (va.z - vb.z));
        }
    }
    va = bias_view(va, g.params.y);
    vb = bias_view(vb, g.params.y);
    let ca = g.proj * vec4<f32>(va, 1.0);
    let cb = g.proj * vec4<f32>(vb, 1.0);
    let half = g.viewport.xy * 0.5;
    let sa = ca.xy / ca.w * half;
    let sb = cb.xy / cb.w * half;
    var d = sb - sa;
    let len = length(d);
    if (len > 1e-5) {
        d = d / len;
    } else {
        d = vec2<f32>(1.0, 0.0);
    }
    let nrm = vec2<f32>(-d.y, d.x);
    let k = corner(vi % 6u);
    let hw = max(in.width * g.viewport.z, 1.0) * 0.5 + 0.75;
    var c = ca;
    if (k.x > 0.5) {
        c = cb;
    }
    let ext = (k.x * 2.0 - 1.0) * hw;
    let off = nrm * k.y * hw + d * ext;
    c = vec4<f32>(c.xy + off / half * c.w, c.z, c.w);
    out.clip = c;
    out.color = in.color;
    out.across = k.y * hw;
    out.along = k.x * len + ext;
    out.hw_dash = vec2<f32>(hw, in.dash * g.viewport.z);
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    let hw = in.hw_dash.x;
    let alpha = clamp(hw - abs(in.across), 0.0, 1.0);
    if (in.hw_dash.y > 0.0) {
        if (fract(in.along / in.hw_dash.y) > 0.55) {
            discard;
        }
    }
    return vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a * alpha);
}

// ---------------------------------------------------------------- points

struct PointIn {
    @location(0) pos: vec3<f32>,
    @location(1) size: f32,
    @location(2) color: vec4<f32>,
};

struct PointOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) @interpolate(flat) radius: f32,
};

@vertex
fn vs_point(@builtin(vertex_index) vi: u32, in: PointIn) -> PointOut {
    var out: PointOut;
    var vp = (g.view * vec4<f32>(in.pos, 1.0)).xyz;
    vp = bias_view(vp, g.params.y * 1.5);
    var c = g.proj * vec4<f32>(vp, 1.0);
    let k = corner(vi % 6u);
    let q = vec2<f32>(k.x * 2.0 - 1.0, k.y);
    let r = max(in.size * g.viewport.z, 2.0) * 0.5 + 1.0;
    let half = g.viewport.xy * 0.5;
    c = vec4<f32>(c.xy + q * r / half * c.w, c.z, c.w);
    out.clip = c;
    out.color = in.color;
    out.local = q * r;
    out.radius = r - 1.0;
    return out;
}

@fragment
fn fs_point(in: PointOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let a = clamp(in.radius + 0.5 - d, 0.0, 1.0);
    if (a <= 0.0) {
        discard;
    }
    // Dark rim so dots read on light and dark surfaces alike.
    let rim = smoothstep(in.radius - 1.6, in.radius - 0.6, d);
    let col = mix(srgb_to_linear(in.color.rgb), srgb_to_linear(in.color.rgb) * 0.12, rim);
    return vec4<f32>(col, in.color.a * a);
}
