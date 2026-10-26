#version 300 es
// Full-screen triangle for the backdrop passes (port of vs_fullscreen in src/shaders/passes.wgsl).
void main() {
    vec2 xy = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(xy * 2.0 - vec2(1.0), 0.0, 1.0);
}
