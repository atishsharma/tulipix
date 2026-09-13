// The lock screen's smoke: domain-warped noise, rising, tinted by three
// colours. The same shader the mockup ran in WebGL
// (docs/mockups/lock-screen.html), with Flutter's coordinate entry point and
// its y axis, which runs down.
//
// Drawn at a third of the window's size by lib/shell/lock/smoke_layer.dart;
// the uniforms are set there by index, in the order they are declared here.

#version 460 core

#include <flutter/runtime_effect.glsl>

precision highp float;

uniform vec2 uSize;     // 0, 1
uniform float uTime;    // 2   seconds
uniform float uEnergy;  // 3   0..1, the track's loudness at the playhead
uniform float uCalm;    // 4   0..1, how much smoke this screen wants
uniform vec3 uA;        // 5..7
uniform vec3 uB;        // 8..10
uniform vec3 uC;        // 11..13
uniform vec2 uPtr;      // 14, 15  pointer, 0..1, y up

out vec4 fragColor;

float hash(vec2 p) {
  p = fract(p * vec2(234.34, 435.345));
  p += dot(p, p + 34.23);
  return fract(p.x * p.y);
}

float noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

float fbm(vec2 p) {
  float v = 0.0;
  float a = 0.5;
  mat2 m = mat2(1.6, 1.2, -1.2, 1.6);
  for (int i = 0; i < 5; i++) {
    v += a * noise(p);
    p = m * p;
    a *= 0.5;
  }
  return v;
}

void main() {
  vec2 fc = FlutterFragCoord().xy;
  vec2 up = vec2(fc.x, uSize.y - fc.y);
  vec2 uv = up / uSize;
  vec2 p = (up - 0.5 * uSize) / uSize.y;
  float t = uTime * 0.05;

  // The pointer stirs it: a swirl that falls off with distance.
  vec2 d = p - (uPtr - 0.5) * vec2(uSize.x / uSize.y, 1.0);
  p += exp(-dot(d, d) * 6.0) * 0.9 * vec2(-d.y, d.x);

  // Smoke rises: sample lower as time goes on, and the shapes climb.
  p.y -= t * 1.6;

  vec2 q = vec2(fbm(p * 1.3 + vec2(0.0, t)), fbm(p * 1.3 + vec2(5.2, 1.3) - t));
  float warp = 2.4 + uEnergy * 1.6;
  vec2 r = vec2(fbm(p * 1.6 + warp * q + vec2(1.7, 9.2) + 0.15 * t),
                fbm(p * 1.6 + warp * q + vec2(8.3, 2.8) - 0.12 * t));
  float f = fbm(p * 1.1 + 2.2 * r);

  vec3 col = mix(uA, uB, clamp(f * f * 2.2, 0.0, 1.0));
  col = mix(col, uC, clamp(length(r) * 0.55, 0.0, 1.0));
  col *= 0.75 + uEnergy * 0.6;

  // Dense near the floor, thinning as it climbs; loudness thickens it.
  float a = smoothstep(0.26, 0.9, f);
  a *= mix(1.2, 0.3, uv.y);
  a *= uCalm * (0.75 + 0.35 * uEnergy);

  fragColor = vec4(col * a, a);  // premultiplied
}
