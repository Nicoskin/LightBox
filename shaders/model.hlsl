// 3D models: lit triangles and the floor grid (lines).
//
// Colours are linear; the render target view is sRGB.

cbuffer Params : register(b0)
{
    row_major float4x4 view_proj; // world → clip, row vectors
    float4 light;                 // xyz: direction towards the key light
    float4 eye;                   // xyz: camera position
    uint   mode;                  // 0 = lit surface, 1 = unlit (grid)
    uint3  pad;
};

struct VSIn
{
    float3 pos : POSITION;
    float3 nrm : NORMAL;
    float4 col : COLOR;
};

struct VSOut
{
    float4 pos  : SV_Position;
    float3 nrm  : NORMAL;
    float4 col  : COLOR;
    float3 wpos : TEXCOORD0;
};

VSOut VSMain(VSIn i)
{
    VSOut o;
    o.pos = mul(float4(i.pos, 1.0), view_proj);
    o.nrm = i.nrm;
    o.col = i.col;
    o.wpos = i.pos;
    return o;
}

float4 PSMain(VSOut i, bool front : SV_IsFrontFace) : SV_Target
{
    if (mode == 1)
        return i.col; // premultiplied grid colour

    float3 n = normalize(i.nrm);
    float3 v = normalize(eye.xyz - i.wpos);
    // Inside faces (open meshes, flipped normals) face the viewer too.
    if (dot(n, v) < 0.0)
        n = -n;
    float3 l = normalize(light.xyz);
    float key = saturate(dot(n, l));
    float fill = saturate(dot(n, normalize(float3(-l.x, 0.35, -l.z)))) * 0.3;
    float sky = lerp(0.18, 0.38, n.y * 0.5 + 0.5); // hemisphere ambient
    float3 h = normalize(l + v);
    float spec = pow(saturate(dot(n, h)), 48.0) * 0.22 * key;
    float3 c = i.col.rgb * (sky + 0.72 * key + fill) + spec;
    return float4(c, 1.0);
}
