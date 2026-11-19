// Closed-form first-order distance of a rounded rectangle with circular (n <= 2.001) or
// continuous corners (superellipse exponent n). Same formula as
// cupertino_geometry::RoundedRect::sdf_approx. p is relative to the rectangle centre.
float rounded_sdf(vec2 p, vec2 hs, float e, float n) {
    vec2 q = abs(p) - (hs - vec2(e));
    if (q.x <= 0.0 && q.y <= 0.0) { return max(q.x, q.y) - e; }
    if (q.x <= 0.0) { return q.y - e; }
    if (q.y <= 0.0) { return q.x - e; }
    if (e <= 0.0) { return length(q); }
    if (n <= 2.001) { return length(q) - e; }
    float u = max(q.x / e, 1e-6);
    float v = max(q.y / e, 1e-6);
    float g = pow(pow(u, n) + pow(v, n), 1.0 / n);
    float grad = sqrt(pow(u, 2.0 * n - 2.0) + pow(v, 2.0 * n - 2.0)) / pow(g, n - 1.0);
    float d = (g - 1.0) / grad * e;
    return max(d, max(q.x, q.y) - e);
}
