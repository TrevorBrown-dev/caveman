// Caveman FX's board cam (crates/caveman-fx/src/overlay.rs, the `pip` settings; src/pip.rs): the skateboard's meshes
// (the deck, the trucks, the wheels: the player's own install's, sent by the trainer) drawn in a corner of the back
// buffer from a fixed viewpoint, turned as the board under the skater is this frame. The deck's underside shows the
// board's live art (t0: the nameless copy the in-world board samples, so whatever plays on the board plays here),
// mapped flat from above as the game maps it; the grip side and the other parts take flat colours. One light, fixed
// in the cam's own frame, so the shading turns with the board.
//
// A backdrop square goes first (flags.w = 1: the vertex position is clip space as given, at the far plane, no light).
// Lighting is done in linear light; the output is encoded like overlay.hlsl's (mode 0 SDR, 1 scRGB, 2 HDR10; nits:
// how bright full white is meant to be). Blending is premultiplied alpha, as the bars'.
//
// Compiled with FXC (shader model 5.0) into pip_vs.dxbc and pip_ps.dxbc; tests/shaders.rs checks the files match
// this source (CAVEMAN_REGEN_SHADERS=1 rewrites them).

cbuffer Pip : register(b0)
{
    row_major float4x4 mvp; // the board's rotation, the cam's view, its projection (row vectors: p * mvp)
    float4 light;           // xyz: towards the light, in the mesh's own frame; w: the ambient share
    float4 colour;          // this part's colour (sRGB) and its alpha
    float4 art;             // x: u per metre of x, y: v per metre of z, z: u at x = 0, w: v at z = 0
    float4 flags;           // x: 1 = the art on faces facing down, y: output mode, z: nits, w: 1 = the backdrop quad
};

Texture2D<float4> art_tex : register(t0);
SamplerState art_sampler : register(s0);

struct VSIn
{
    float3 pos : POSITION;
    float3 normal : NORMAL;
};

struct PSIn
{
    float4 pos : SV_Position;
    float3 mpos : TEXCOORD0;
    float3 normal : TEXCOORD1;
};

PSIn vs_main(VSIn i)
{
    PSIn o;
    if (flags.w > 0.5)
    {
        o.pos = float4(i.pos.xy, 1.0, 1.0);
        o.mpos = i.pos;
        o.normal = float3(0.0, 1.0, 0.0);
        return o;
    }
    o.pos = mul(float4(i.pos, 1.0), mvp);
    o.mpos = i.pos;
    o.normal = i.normal;
    return o;
}

float3 srgb_to_linear(float3 c)
{
    float3 lo = c / 12.92;
    float3 hi = pow(max((c + 0.055) / 1.055, 0.0), 2.4);
    return float3(c.x <= 0.04045 ? lo.x : hi.x, c.y <= 0.04045 ? lo.y : hi.y, c.z <= 0.04045 ? lo.z : hi.z);
}

float3 linear_to_srgb(float3 l)
{
    float3 lo = l * 12.92;
    float3 hi = 1.055 * pow(max(l, 0.0), 1.0 / 2.4) - 0.055;
    return float3(l.x <= 0.0031308 ? lo.x : hi.x, l.y <= 0.0031308 ? lo.y : hi.y, l.z <= 0.0031308 ? lo.z : hi.z);
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
    float mode = flags.y;
    float nits = flags.z;
    float alpha = saturate(colour.a);
    float3 lin;
    if (flags.w > 0.5)
    {
        lin = srgb_to_linear(saturate(colour.rgb));
    }
    else
    {
        float3 n = normalize(i.normal);
        float shade = light.w + (1.0 - light.w) * saturate(dot(n, normalize(light.xyz)));
        float3 base = srgb_to_linear(saturate(colour.rgb));
        if (flags.x > 0.5 && n.y < -0.15)
        {
            // the art: an sRGB texture, so the sample is linear already
            float2 uv = float2(art.z + i.mpos.x * art.x, art.w - i.mpos.z * art.y);
            base = art_tex.Sample(art_sampler, uv).rgb;
        }
        lin = base * shade;
    }
    float3 outc;
    if (mode > 1.5)
    {
        outc = pq_encode(bt709_to_bt2020(lin) * (nits / 10000.0));
    }
    else if (mode > 0.5)
    {
        outc = lin * (nits / 80.0);
    }
    else
    {
        outc = linear_to_srgb(saturate(lin));
    }
    return float4(outc * alpha, alpha);
}
