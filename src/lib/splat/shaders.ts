// WebGL2 3D Gaussian Splatting Shaders (Mathematically Exact EWA Splatting)

export const splatVertexShader = `#version 300 es
precision highp float;
precision highp int;

// Instanced quad corner [-1, 1]
layout(location = 0) in vec2 a_quadPos;
// Sorted Splat Index
layout(location = 1) in uint a_splatIndex;

uniform mat4 u_view;
uniform mat4 u_projection;
uniform vec2 u_focal;      // (fx, fy) in pixels
uniform vec2 u_viewport;   // (width, height) in pixels
uniform float u_splatScale;
uniform highp int u_renderMode;   // 0 = Splat, 1 = Point Cloud

uniform highp sampler2D u_texCenters;
uniform highp sampler2D u_texRotations;
uniform highp sampler2D u_texScales;
uniform highp sampler2D u_texColors;
uniform int u_texWidth;

out vec4 v_color;
out vec2 v_quadCoord;

ivec2 getTexCoord(uint index) {
    int x = int(index) % u_texWidth;
    int y = int(index) / u_texWidth;
    return ivec2(x, y);
}

mat3 computeCov3D(vec3 scale, vec4 q) {
    // Quaternion: (qw, qx, qy, qz)
    float r = q.x;
    float x = q.y;
    float y = q.z;
    float z = q.w;

    // GLSL mat3 takes columns: mat3(col0, col1, col2)
    mat3 R = mat3(
        1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + r * z),       2.0 * (x * z - r * y),
        2.0 * (x * y - r * z),       1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + r * x),
        2.0 * (x * z + r * y),       2.0 * (y * z - r * x),       1.0 - 2.0 * (x * x + y * y)
    );

    mat3 S = mat3(
        scale.x, 0.0,     0.0,
        0.0,     scale.y, 0.0,
        0.0,     0.0,     scale.z
    );

    mat3 M = R * S;
    return M * transpose(M);
}

void main() {
    ivec2 coord = getTexCoord(a_splatIndex);
    vec4 centerAndOpacity = texelFetch(u_texCenters, coord, 0);
    vec4 rotation = texelFetch(u_texRotations, coord, 0);
    vec3 scale = texelFetch(u_texScales, coord, 0).xyz;
    vec4 color = texelFetch(u_texColors, coord, 0);
    color.a = centerAndOpacity.w;

    v_color = color;
    v_quadCoord = a_quadPos;

    // View-space position
    vec4 viewPos = u_view * vec4(centerAndOpacity.xyz, 1.0);
    if (viewPos.z >= -0.01) {
        // Behind camera
        gl_Position = vec4(0.0, 0.0, 2.0, 1.0);
        return;
    }

    vec4 projCenter = u_projection * viewPos;
    float clip = 1.3 * projCenter.w;
    if (projCenter.z < -projCenter.w || projCenter.x < -clip || projCenter.x > clip || projCenter.y < -clip || projCenter.y > clip) {
        gl_Position = vec4(0.0, 0.0, 2.0, 1.0);
        return;
    }

    if (u_renderMode == 1) {
        // Point Cloud mode
        vec2 offset = a_quadPos * (u_splatScale * 4.0 / u_viewport);
        gl_Position = vec4(projCenter.xy + offset * projCenter.w, projCenter.zw);
        return;
    }

    // 3D Covariance Matrix
    mat3 Vrk = computeCov3D(scale * u_splatScale, rotation);

    // Camera space view matrix (3x3)
    mat3 W = mat3(u_view);

    // Jacobian of perspective projection in camera coordinates:
    // (u, v) = (fx * x / z, fy * y / z)
    mat3 J = mat3(
        u_focal.x / viewPos.z, 0.0, 0.0,
        0.0, u_focal.y / viewPos.z, 0.0,
        -(u_focal.x * viewPos.x) / (viewPos.z * viewPos.z), -(u_focal.y * viewPos.y) / (viewPos.z * viewPos.z), 0.0
    );

    mat3 T = J * W;
    mat3 cov2D = T * Vrk * transpose(T);

    // Low-pass filter for anti-aliasing
    float a = cov2D[0][0] + 0.3;
    float b = cov2D[0][1];
    float d = cov2D[1][1] + 0.3;

    float det = a * d - b * b;
    if (det <= 0.0) {
        gl_Position = vec4(0.0, 0.0, 2.0, 1.0);
        return;
    }

    // Compute eigenvalues of 2D covariance
    float mid = 0.5 * (a + d);
    float radius = sqrt(max(0.1, mid * mid - det));
    float lambda1 = mid + radius;
    float lambda2 = max(0.1, mid - radius);

    // Eigenvectors
    vec2 v = normalize(vec2(b, lambda1 - a));
    if (length(vec2(b, lambda1 - a)) < 0.0001) {
        v = vec2(1.0, 0.0);
    }
    vec2 v_perp = vec2(-v.y, v.x);

    // 3-sigma bounding ellipse (covers 99.7% of Gaussian distribution)
    vec2 scale2d = 3.0 * sqrt(vec2(lambda1, lambda2));

    // Convert pixel offset to NDC
    vec2 ndcOffset = 2.0 * (a_quadPos.x * scale2d.x * v + a_quadPos.y * scale2d.y * v_perp) / u_viewport;

    gl_Position = vec4(projCenter.xy + ndcOffset * projCenter.w, projCenter.zw);
}
`;

export const splatFragmentShader = `#version 300 es
precision highp float;
precision highp int;

in vec4 v_color;
in vec2 v_quadCoord;

uniform highp int u_renderMode; // 0 = Splat, 1 = Point Cloud

out vec4 fragColor;

void main() {
    float distSq = dot(v_quadCoord, v_quadCoord);
    if (distSq > 1.0) {
        discard;
    }

    if (u_renderMode == 1) {
        // Point Cloud circular dot
        fragColor = vec4(v_color.rgb, 0.95);
        return;
    }

    // Gaussian radial falloff: exp(-0.5 * (3.0 * dist)^2) = exp(-4.5 * distSq)
    float power = -4.5 * distSq;
    float alpha = exp(power) * v_color.a;

    if (alpha < 1.0 / 255.0) {
        discard;
    }

    // Premultiplied alpha output
    fragColor = vec4(v_color.rgb * alpha, alpha);
}
`;

export const gridVertexShader = `#version 300 es
precision highp float;
precision highp int;

layout(location = 0) in vec3 a_position;

uniform mat4 u_view;
uniform mat4 u_projection;
uniform float u_gridSize;

out vec3 v_worldPos;

void main() {
    v_worldPos = a_position * u_gridSize;
    gl_Position = u_projection * u_view * vec4(v_worldPos, 1.0);
}
`;

export const gridFragmentShader = `#version 300 es
precision highp float;
precision highp int;

in vec3 v_worldPos;
out vec4 fragColor;

void main() {
    vec2 coord = v_worldPos.xz;
    vec2 grid = abs(fract(coord - 0.5) - 0.5) / fwidth(coord);
    float line = min(grid.x, grid.y);
    float c = 1.0 - min(line, 1.0);

    float dist = length(coord);
    float fade = clamp(1.0 - dist / 8.0, 0.0, 1.0);

    if (c * fade < 0.01) discard;
    fragColor = vec4(0.4, 0.45, 0.55, c * fade * 0.35);
}
`;
