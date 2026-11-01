// Caveman FX's overlay shaders (crates/caveman-fx/src/overlay.rs): coloured quads drawn into the game's back buffer
// just before it is presented.
//
// The vertex positions arrive already in clip space (the CPU projects them: src/bars.rs), so the vertex shader only
// passes things through (dpos too: the point in the depth copy's camera, for the depth test). The pixel
// shader draws one of three things. `local` holds the pixel's place in the bar's own frame, in metres: x along the bar
// from its base, y across it; z is the bar's length and w its half width.
//
// - extra.y = 1: a screen-space quad (the test rectangle), added to the picture;
// - extra.z = 1: the solid style: a crisp, opaque rectangle (a box distance, about a pixel of anti-aliasing, square
//   ends), with an optional darker outline extra.w metres wide around it and an optional soft end (the last extra.x
//   metres fade out);
// - else the glow style: a capsule with a glow extra.x metres wide and a white-hot centre line, added to the picture.
//
// Bars are two-tone: from tip.w metres along (past the end: never) they take the colour tip.rgb.
//
// Blending is premultiplied alpha (source ONE, destination INV_SRC_ALPHA): the solid style writes (colour x coverage,
// coverage) and so replaces what is under it (exactly, in every output encoding: the value written is already
// encoded); the added looks write (colour x coverage, 0) and keep their additive glow.
//
// Output: the root constants say what the back buffer holds. 0: SDR (sRGB-encoded values, written as they are);
// 1: scRGB (linear BT.709, 1.0 = 80 nits); 2: HDR10 (BT.2020 primaries, the PQ curve). `nits` is how bright the
// colour's full value is meant to be on an HDR display.
//
// Depth (depth_on = 1): a copy of the game's own depth buffer is bound as t0. A bar pixel is hidden when the game's
// scene at that pixel is more than depth_bias metres nearer the camera than the bar: the skater's legs and shoes,
// the deck, the wheels. The bar's point is taken in the clip space of the camera the copy was rendered with (dpos:
// the same as pos when the copy is the frame's own; an older copy's camera when the newest is late), so a late copy
// still lines up with the world. depth_map takes that point's normalised device coordinates to the depth texel (the
// depth is at the render resolution, below the output's with an upscaler, and the frame rendered into a viewport of
// it); depth_lin turns a depth value d into view depth in metres, y / (d - x) (reversed Z: x = 0, y = the near plane;
// standard: x = 1, y = -near), zw the last texel.
//
// Compiled with FXC (shader model 5.0, DXBC: Direct3D 12 takes it as it is) into overlay_vs.dxbc and overlay_ps.dxbc;
// tests/shaders.rs checks the files match this source (CAVEMAN_REGEN_SHADERS=1 rewrites them).

cbuffer Output : register(b0)
{
    float mode;
    float nits;
    float depth_on;
    float depth_bias;
    float4 depth_map;
    float4 depth_lin;
    // the blink counter: this frame's pair of counts in `counts` (8 bytes each), and whether to count
    float count_slot;
    float count_on;
    float2 count_pad;
};

Texture2D<float> scene_depth : register(t0);
// the bars' pixels tested against the depth and hidden by it, one in 16 (every 4th column of every 4th row): the
// overlay reads them back a few frames later to find blinks (src/diag.rs). u1: shader model 5 puts a pixel shader's UAVs
// after its render target (u0).
RWByteAddressBuffer counts : register(u1);

struct VSIn
{
    float4 pos : POSITION;
    float4 local : TEXCOORD0;
    float4 color : COLOR0;
    float4 extra : TEXCOORD1;
    float4 tip : TEXCOORD2;
    float4 dpos : TEXCOORD3;
};

struct PSIn
{
    float4 pos : SV_Position;
    float4 local : TEXCOORD0;
    float4 color : COLOR0;
    float4 extra : TEXCOORD1;
    float4 tip : TEXCOORD3;
    float4 dpos : TEXCOORD4;
};

PSIn vs_main(VSIn i)
{
    PSIn o;
    o.pos = i.pos;
    o.local = i.local;
    o.color = i.color;
    o.extra = i.extra;
    o.tip = i.tip;
    o.dpos = i.dpos;
    return o;
}

float3 srgb_to_linear(float3 c)
{
    float3 lo = c / 12.92;
    float3 hi = pow(max((c + 0.055) / 1.055, 0.0), 2.4);
    return float3(c.x <= 0.04045 ? lo.x : hi.x, c.y <= 0.04045 ? lo.y : hi.y, c.z <= 0.04045 ? lo.z : hi.z);
}

float3 bt709_to_bt2020(float3 c)
{
    return float3(
        dot(float3(0.627402, 0.329292, 0.043306), c),
        dot(float3(0.069095, 0.919544, 0.011360), c),
        dot(float3(0.016394, 0.088028, 0.895578), c));
}

float3 pq_encode(float3 l)
{
    // l: linear light / 10000 nits
    const float m1 = 0.1593017578125;
    const float m2 = 78.84375;
    const float c1 = 0.8359375;
    const float c2 = 18.8515625;
    const float c3 = 18.6875;
    float3 p = pow(saturate(l), m1);
    return pow((c1 + c2 * p) / (1.0 + c3 * p), m2);
}

float4 ps_main(PSIn i) : SV_Target
{
    float3 rgb = saturate(i.color.rgb);
    float len = i.local.z;
    float r = max(i.local.w, 1e-4);
    // both shapes' distances, and their screen-space rates, before any branch (derivatives need every pixel)
    float2 q = float2(abs(i.local.x - 0.5 * len) - 0.5 * len, abs(i.local.y) - r);
    float box = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
    float box_aa = max(fwidth(box), 1e-5) * 0.5;
    float t = clamp(i.local.x, 0.0, len);
    float2 c = float2(i.local.x - t, i.local.y);
    float cap = length(c) - r;
    float cap_aa = max(fwidth(cap), 1e-5);
    // the tip: the outer part from tip.w metres on takes the tip colour (past the end: none)
    float tip_aa = max(fwidth(i.local.x), 1e-5) * 0.5;
    rgb = lerp(rgb, saturate(i.tip.rgb), smoothstep(i.tip.w - tip_aa, i.tip.w + tip_aa, i.local.x));

    float cover;
    float opaque;
    if (i.extra.y > 0.5)
    {
        cover = i.color.a;
        opaque = 0.0;
    }
    else if (i.extra.z > 0.5)
    {
        float ol = max(i.extra.w, 0.0);
        cover = (1.0 - smoothstep(ol - box_aa, ol + box_aa, box)) * i.color.a;
        if (i.extra.x > 0.0)
        {
            // a soft end: the last extra.x metres fade out
            cover *= 1.0 - smoothstep(len - i.extra.x, len, i.local.x);
        }
        if (ol > 0.0)
        {
            rgb = lerp(rgb, rgb * 0.18, smoothstep(-box_aa, box_aa, box));
        }
        opaque = 1.0;
    }
    else
    {
        float core = 1.0 - smoothstep(-cap_aa, cap_aa, cap);
        float glow_width = max(i.extra.x, 1e-4);
        float glow = 0.55 * exp(-3.0 * max(cap, 0.0) / glow_width);
        cover = saturate(max(core, glow)) * i.color.a;
        // a white-hot centre line
        float centre = saturate(1.0 - length(c) / r);
        rgb = lerp(rgb, float3(1.0, 1.0, 1.0), 0.55 * centre * centre * core);
        opaque = 0.0;
    }
    if (depth_on > 0.5 && i.extra.y < 0.5 && i.dpos.w > 0.05)
    {
        // where this point of the bar was in the frame the depth copy was rendered (the frame's own: dpos = pos), and
        // how far from that frame's camera
        float2 ndc = i.dpos.xy / i.dpos.w;
        int2 texel = int2(clamp(ndc * depth_map.xy + depth_map.zw, float2(0.0, 0.0), depth_lin.zw));
        float d = scene_depth.Load(int3(texel, 0));
        float scene = depth_lin.y / (d - depth_lin.x);
        bool hidden = scene > 0.0 && scene + depth_bias < i.dpos.w;
        uint2 px = uint2(i.pos.xy);
        if (count_on > 0.5 && cover > 0.5 && (px.x & 3) == 0 && (px.y & 3) == 0)
        {
            uint at = (uint)count_slot * 8;
            counts.InterlockedAdd(at, 1);
            if (hidden)
            {
                counts.InterlockedAdd(at + 4, 1);
            }
        }
        if (hidden)
        {
            cover = 0.0;
        }
    }
    float3 outc;
    if (mode > 1.5)
    {
        outc = pq_encode(bt709_to_bt2020(srgb_to_linear(rgb)) * (nits / 10000.0));
    }
    else if (mode > 0.5)
    {
        outc = srgb_to_linear(rgb) * (nits / 80.0);
    }
    else
    {
        outc = rgb;
    }
    return float4(outc * cover, cover * opaque);
}
