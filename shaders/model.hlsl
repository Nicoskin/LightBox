// 3D models: lit triangles and the floor grid (lines).
//
// Colours are linear; the render target view is sRGB.

cbuffer Params : register(b0)
{
    row_major float4x4 view_proj; // world → clip, row vectors
    float4 light;                 // xyz: direction towards the key light
    float4 eye;                   // xyz: camera position
    uint   mode;                  // 0 = lit surface, 1 = unlit (grid), 2 = wireframe
    uint3  pad;
    float4 section;               // x = 1: cut away everything above y = section.y
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
    if (section.x > 0.5 && i.wpos.y > section.y)
        discard;
    if (mode == 1)
        return i.col; // premultiplied grid colour
    if (mode == 2)
    {
        // Wireframe lines over the surface, not over a cut.
        if (section.x > 0.5 && dot(i.nrm, eye.xyz - i.wpos) < 0.0)
            discard;
        return float4(0.0, 0.0, 0.0, 0.55);
    }

    float3 n = normalize(i.nrm);
    float3 v = normalize(eye.xyz - i.wpos);
    // Inside faces (open meshes, flipped normals) face the viewer too. Seen
    // through a cut they are the inside of the part: hatched in red, which
    // sets it apart from a surface of any colour.
    if (dot(n, v) < 0.0)
    {
        n = -n;
        if (section.x > 0.5)
        {
            bool stripe = frac((i.pos.x + i.pos.y) / 10.0) < 0.5;
            return float4(stripe ? float3(0.70, 0.06, 0.03) : float3(0.30, 0.025, 0.012), 1.0);
        }
    }
    float3 l = normalize(light.xyz);
    float key = saturate(dot(n, l));
    float fill = saturate(dot(n, normalize(float3(-l.x, 0.35, -l.z)))) * 0.3;
    float sky = lerp(0.18, 0.38, n.y * 0.5 + 0.5); // hemisphere ambient
    float3 h = normalize(l + v);
    float spec = pow(saturate(dot(n, h)), 48.0) * 0.22 * key;
    float3 c = i.col.rgb * (sky + 0.72 * key + fill) + spec;
    // Premultiplied: see-through parts are blended (alpha < 1).
    return float4(c * i.col.a, i.col.a);
}

// G-code toolpaths: one instance per extruded segment, expanded here into
// a bar (top and two sides, 18 vertices) of the line's width.
struct Seg
{
    float3 a;
    float3 b;
    uint   kind;
    float  width;
};
StructuredBuffer<Seg> segs : register(t0);

// Line colours by type (linear), close to the slicers' own.
static const float3 PALETTE[11] =
{
    float3(0.55, 0.55, 0.58), // other
    float3(1.00, 0.42, 0.05), // outer wall
    float3(1.00, 0.78, 0.12), // inner wall
    float3(0.62, 0.10, 0.08), // sparse infill
    float3(0.45, 0.10, 0.62), // solid infill
    float3(0.92, 0.18, 0.25), // top surface
    float3(0.25, 0.30, 0.85), // bottom surface
    float3(0.10, 0.70, 0.20), // support
    float3(0.10, 0.65, 0.70), // skirt / brim
    float3(0.20, 0.40, 0.95), // bridge
    float3(0.10, 0.35, 0.90), // overhang
};

VSOut VSPath(uint vid : SV_VertexID, uint iid : SV_InstanceID)
{
    Seg s = segs[iid];
    float3 d = s.b - s.a;
    float len = length(d);
    float3 dir = len > 1e-6 ? d / len : float3(1, 0, 0);
    float3 side = cross(float3(0, 1, 0), dir);
    side = dot(side, side) > 1e-8 ? normalize(side) : float3(1, 0, 0);
    float hw = s.width * 0.5;
    float h = s.width * 0.45;

    uint q = vid / 6;          // 0 top, 1 left side, 2 right side
    uint k = vid % 6;
    uint corner = k == 0 ? 0 : (k == 1 || k == 4) ? 1 : (k == 2 || k == 3) ? 2 : 3;
    float along = (float)(corner & 1);
    float across = (float)(corner >> 1);
    float3 p = lerp(s.a, s.b, along);
    float3 pos, n;
    if (q == 0)
    {
        pos = p + side * (across * 2.0 - 1.0) * hw;
        n = float3(0, 1, 0);
    }
    else
    {
        float sgn = q == 1 ? -1.0 : 1.0;
        pos = p + side * sgn * hw - float3(0, across * h, 0);
        n = side * sgn;
    }
    VSOut o;
    o.pos = mul(float4(pos, 1.0), view_proj);
    o.nrm = n;
    o.col = float4(PALETTE[min(s.kind, 10u)], 1.0);
    o.wpos = pos;
    return o;
}
