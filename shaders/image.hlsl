// Image quad: draws the visible part of the image with orientation,
// transparency background, a free rotation and (later) channel / pixel-grid
// modes.
//
// Colour pipeline: the texture is R8G8B8A8_UNORM_SRGB holding *premultiplied
// linear* colour (encoded back to sRGB per channel), so sampling returns
// premultiplied linear RGBA and filtering / mipmapping are correct. The render
// target view is *_SRGB, so blending happens in linear light too.

cbuffer Params : register(b0)
{
    float4 dst_rect;       // visible quad on screen (unrotated), pixels: x0, y0, x1, y1
    float4 uv_rect;        // matching display-space UV:      u0, v0, u1, v1
    float4 bg_color;       // solid background, premultiplied linear
    float2 viewport;       // render target size, pixels
    float2 checker_origin; // screen position the checkerboard is anchored to
    float  checker_size;   // checker cell size, screen pixels
    uint   orientation;    // EXIF orientation 1..8
    uint   bg_mode;        // 0 = none, 1 = checkerboard, 2 = solid colour
    uint   channel_mode;   // 0 = RGBA, 1..3 = R, G, B as grey, 4 = alpha as a mask
    uint   grid;           // 1 = outline every texel (pixel inspector)
    float  opacity;        // layer opacity (fades, page turns)
    float2 rot;            // rotation about `pivot`: (cos, sin); (1, 0) = none
    float4 tex_rect;       // content part of the texture: offset xy, size zw (tiles)
    float2 pivot;          // rotation centre, screen pixels
    uint   edges;          // sides of this part that are image edges: 1 left, 2 top, 4 right, 8 bottom
    uint   pad;
};

Texture2D<float4> image : register(t0);
SamplerState      samp  : register(s0);

struct VSOut
{
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
    float2 d   : TEXCOORD1; // display-space UV of the part (edge anti-aliasing)
};

// Maps display-space UV (after orientation) to texture UV.
float2 orient(float2 d, uint o)
{
    switch (o)
    {
    case 2:  return float2(1.0 - d.x, d.y);         // mirror horizontal
    case 3:  return float2(1.0 - d.x, 1.0 - d.y);   // rotate 180
    case 4:  return float2(d.x, 1.0 - d.y);         // mirror vertical
    case 5:  return float2(d.y, d.x);               // transpose
    case 6:  return float2(d.y, 1.0 - d.x);         // rotate 90 CW
    case 7:  return float2(1.0 - d.y, 1.0 - d.x);   // transverse
    case 8:  return float2(1.0 - d.y, d.x);         // rotate 90 CCW
    default: return d;
    }
}

VSOut VSMain(uint id : SV_VertexID)
{
    // Triangle strip: (0,0) (1,0) (0,1) (1,1)
    float2 c = float2(id & 1, id >> 1);
    float2 p = lerp(dst_rect.xy, dst_rect.zw, c);
    float2 d = lerp(uv_rect.xy, uv_rect.zw, c);
    float2 q = p - pivot;
    p = pivot + float2(q.x * rot.x - q.y * rot.y, q.x * rot.y + q.y * rot.x);

    VSOut o;
    o.pos = float4(p.x / viewport.x * 2.0 - 1.0, 1.0 - p.y / viewport.y * 2.0, 0.0, 1.0);
    o.uv = tex_rect.xy + orient(d, orientation) * tex_rect.zw;
    o.d = d;
    return o;
}

// Pixel grid: darkens (or, on dark pixels, lightens) a one-pixel line along
// texel boundaries.
float4 grid_line(float2 uv, float4 c)
{
    if (grid == 0)
        return c;
    float w, h;
    image.GetDimensions(w, h);
    float2 t = uv * float2(w, h);
    float2 d = abs(frac(t + 0.5) - 0.5) / max(fwidth(t), 1e-6);
    float line_a = saturate(1.0 - min(d.x, d.y)) * 0.55;
    float lum = dot(c.rgb, float3(0.2126, 0.7152, 0.0722));
    float4 ink = lum > 0.18 ? float4(0.0, 0.0, 0.0, 1.0) : float4(0.55, 0.55, 0.55, 1.0);
    return lerp(c, ink, line_a);
}

// Distance in screen pixels from the pixel centre to where `v` reaches 0.
float edge_px(float v, float dx, float dy)
{
    return v / max(length(float2(dx, dy)), 1e-7);
}

float4 PSMain(VSOut i) : SV_Target
{
    float4 c = image.Sample(samp, i.uv); // premultiplied, linear

    if (channel_mode == 4)
    {
        // Alpha as an opaque black / white mask (alpha is not gamma-encoded:
        // show its value as an sRGB level).
        float v = pow(c.a, 2.2);
        c = float4(v, v, v, 1.0);
    }
    else if (channel_mode != 0)
    {
        float3 s = c.a > 0.0 ? c.rgb / c.a : 0.0;
        float v = channel_mode == 1 ? s.r : (channel_mode == 2 ? s.g : s.b);
        c = float4(v, v, v, 1.0) * c.a;
    }

    // A turned image has slanted edges: fade the outermost pixel row.
    float cover = 1.0;
    if (rot.y != 0.0)
    {
        float2 dx = ddx(i.d), dy = ddy(i.d);
        if (edges & 1) cover *= saturate(edge_px(i.d.x, dx.x, dy.x));
        if (edges & 2) cover *= saturate(edge_px(i.d.y, dx.y, dy.y));
        if (edges & 4) cover *= saturate(edge_px(1.0 - i.d.x, dx.x, dy.x));
        if (edges & 8) cover *= saturate(edge_px(1.0 - i.d.y, dx.y, dy.y));
    }

    if (bg_mode == 0)
        return grid_line(i.uv, c) * opacity * cover; // transparent areas show the backdrop

    float4 bg = bg_color;
    if (bg_mode == 1)
    {
        // #FFFFFF / #CCCCCC (0.8 sRGB = 0.6038 linear), fixed size in screen pixels.
        int2 cell = int2(floor((i.pos.xy - checker_origin) / checker_size));
        float v = ((cell.x + cell.y) & 1) ? 0.6038 : 1.0;
        bg = float4(v, v, v, 1.0);
    }
    return grid_line(i.uv, c + bg * (1.0 - c.a)) * opacity * cover;
}
