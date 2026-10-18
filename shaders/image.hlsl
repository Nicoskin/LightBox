// Image quad: draws the visible part of the image with orientation,
// transparency background and (later) channel / pixel-grid modes.
//
// Colour pipeline: the texture is R8G8B8A8_UNORM_SRGB holding *premultiplied
// linear* colour (encoded back to sRGB per channel), so sampling returns
// premultiplied linear RGBA and filtering / mipmapping are correct. The render
// target view is *_SRGB, so blending happens in linear light too.

cbuffer Params : register(b0)
{
    float4 dst_rect;       // visible quad on screen, pixels: x0, y0, x1, y1
    float4 uv_rect;        // matching display-space UV:      u0, v0, u1, v1
    float4 bg_color;       // solid background, premultiplied linear
    float2 viewport;       // render target size, pixels
    float2 checker_origin; // screen position the checkerboard is anchored to
    float  checker_size;   // checker cell size, screen pixels
    uint   orientation;    // EXIF orientation 1..8
    uint   bg_mode;        // 0 = none, 1 = checkerboard, 2 = solid colour
    uint   channel_mode;   // 0 = RGBA (reserved: 1..4 = R, G, B, A)
    uint   grid;           // reserved: pixel grid
    float  opacity;        // layer opacity (fades, page turns)
    uint2  pad;
};

Texture2D<float4> image : register(t0);
SamplerState      samp  : register(s0);

struct VSOut
{
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
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

    VSOut o;
    o.pos = float4(p.x / viewport.x * 2.0 - 1.0, 1.0 - p.y / viewport.y * 2.0, 0.0, 1.0);
    o.uv = orient(d, orientation);
    return o;
}

float4 PSMain(VSOut i) : SV_Target
{
    float4 c = image.Sample(samp, i.uv); // premultiplied, linear

    if (bg_mode == 0)
        return c * opacity; // transparent areas show the dimmed backdrop

    float4 bg = bg_color;
    if (bg_mode == 1)
    {
        // #FFFFFF / #CCCCCC (0.8 sRGB = 0.6038 linear), fixed size in screen pixels.
        int2 cell = int2(floor((i.pos.xy - checker_origin) / checker_size));
        float v = ((cell.x + cell.y) & 1) ? 0.6038 : 1.0;
        bg = float4(v, v, v, 1.0);
    }
    return (c + bg * (1.0 - c.a)) * opacity;
}
