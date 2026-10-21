// Caveman FX's overlay shaders (crates/caveman-fx/src/overlay.rs): coloured quads drawn into the game's back buffer
// just before it is presented, blended additively (a glow).
//
// The vertex positions arrive already in clip space (the CPU projects them: src/bars.rs), so the vertex shader only
// passes things through. The pixel shader draws either a solid quad (the test rectangle) or a glowing capsule (a bar):
// `local` holds the pixel's place in the bar's own frame, in metres: x along the bar from its base, y across it; z is
// the bar's length and w its radius. extra.x is the glow's width (metres), extra.y 1 for a solid quad.
//
// Output: the root constants say what the back buffer holds. 0: SDR (sRGB-encoded values, written as they are);
// 1: scRGB (linear BT.709, 1.0 = 80 nits); 2: HDR10 (BT.2020 primaries, the PQ curve). `nits` is how bright the
// colour's full value is meant to be on an HDR display.
//
// Compiled with FXC (shader model 5.0, DXBC: Direct3D 12 takes it as it is) into overlay_vs.dxbc and overlay_ps.dxbc;
// tests/shaders.rs checks the files match this source (CAVEMAN_REGEN_SHADERS=1 rewrites them).

cbuffer Output : register(b0)
{
    float mode;
    float nits;
    float pad0;
    float pad1;
};

struct VSIn
{
    float4 pos : POSITION;
    float4 local : TEXCOORD0;
    float4 color : COLOR0;
    float4 extra : TEXCOORD1;
};

struct PSIn
{
    float4 pos : SV_Position;
    float4 local : TEXCOORD0;
    float4 color : COLOR0;
    float4 extra : TEXCOORD1;
};

PSIn vs_main(VSIn i)
{
    PSIn o;
    o.pos = i.pos;
    o.local = i.local;
    o.color = i.color;
    o.extra = i.extra;
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
    float alpha;
    if (i.extra.y > 0.5)
    {
        alpha = i.color.a;
    }
    else
    {
        float len = i.local.z;
        float r = max(i.local.w, 1e-4);
        float t = clamp(i.local.x, 0.0, len);
        float2 q = float2(i.local.x - t, i.local.y);
        float d = length(q) - r;
        float aa = max(fwidth(d), 1e-5);
        float core = 1.0 - smoothstep(-aa, aa, d);
        float glow_width = max(i.extra.x, 1e-4);
        float glow = 0.55 * exp(-3.0 * max(d, 0.0) / glow_width);
        alpha = saturate(max(core, glow)) * i.color.a;
        // a white-hot centre line
        float centre = saturate(1.0 - length(q) / r);
        rgb = lerp(rgb, float3(1.0, 1.0, 1.0), 0.55 * centre * centre * core);
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
    return float4(outc, alpha);
}
