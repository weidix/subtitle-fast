use std::cmp::Ordering;
use std::f32::consts::SQRT_2;

pub fn resize_average(
    pixels: &[f32],
    width: usize,
    height: usize,
    new_width: usize,
    new_height: usize,
) -> Vec<f32> {
    assert_eq!(pixels.len(), width * height);
    if width == 0 || height == 0 || new_width == 0 || new_height == 0 {
        return vec![0.0; new_width * new_height];
    }
    let scale_x = width as f32 / new_width as f32;
    let scale_y = height as f32 / new_height as f32;
    let mut output = vec![0.0f32; new_width * new_height];
    for ny in 0..new_height {
        let src_y0 = (ny as f32 * scale_y).floor() as isize;
        let src_y1 = (((ny + 1) as f32 * scale_y).ceil() as isize).min(height as isize);
        for nx in 0..new_width {
            let src_x0 = (nx as f32 * scale_x).floor() as isize;
            let src_x1 = (((nx + 1) as f32 * scale_x).ceil() as isize).min(width as isize);
            let mut sum = 0.0f32;
            let mut count = 0;
            for sy in src_y0.max(0)..src_y1.max(src_y0 + 1) {
                for sx in src_x0.max(0)..src_x1.max(src_x0 + 1) {
                    let idx = sy as usize * width + sx as usize;
                    sum += pixels[idx];
                    count += 1;
                }
            }
            let value = if count == 0 { 0.0 } else { sum / count as f32 };
            output[ny * new_width + nx] = value;
        }
    }
    output
}

pub fn gaussian_blur_3x3(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(pixels.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let kernel = [[1.0f32, 2.0, 1.0], [2.0, 4.0, 2.0], [1.0, 2.0, 1.0]];
    let mut output = vec![0.0f32; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0;
            let mut weight = 0.0;
            for (ky, row) in kernel.iter().enumerate() {
                for (kx, &w) in row.iter().enumerate() {
                    let oy = y as isize + ky as isize - 1;
                    let ox = x as isize + kx as isize - 1;
                    if oy < 0 || ox < 0 || oy >= height as isize || ox >= width as isize {
                        continue;
                    }
                    let idx = oy as usize * width + ox as usize;
                    sum += pixels[idx] * w;
                    weight += w;
                }
            }
            output[y * width + x] = if weight == 0.0 { 0.0 } else { sum / weight };
        }
    }
    output
}

pub fn sobel_magnitude_into(pixels: &[f32], width: usize, height: usize, output: &mut Vec<f32>) {
    assert_eq!(pixels.len(), width * height);
    output.clear();
    if width == 0 || height == 0 {
        return;
    }
    output.resize(pixels.len(), 0.0);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let idx = y * width + x;
            let gx = pixels[(y - 1) * width + (x + 1)]
                + 2.0 * pixels[y * width + (x + 1)]
                + pixels[(y + 1) * width + (x + 1)]
                - pixels[(y - 1) * width + (x - 1)]
                - 2.0 * pixels[y * width + (x - 1)]
                - pixels[(y + 1) * width + (x - 1)];
            let gy = pixels[(y + 1) * width + (x - 1)]
                + 2.0 * pixels[(y + 1) * width + x]
                + pixels[(y + 1) * width + (x + 1)]
                - pixels[(y - 1) * width + (x - 1)]
                - 2.0 * pixels[(y - 1) * width + x]
                - pixels[(y - 1) * width + (x + 1)];
            output[idx] = gx.abs() + gy.abs();
        }
    }
}

pub fn sobel_magnitude(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut output = Vec::new();
    sobel_magnitude_into(pixels, width, height, &mut output);
    output
}

pub fn normalize(values: &mut [f32]) {
    if values.is_empty() {
        return;
    }
    let mut max_value = values[0];
    for &v in values.iter().skip(1) {
        if v > max_value {
            max_value = v;
        }
    }
    if max_value <= f32::EPSILON {
        return;
    }
    for value in values.iter_mut() {
        *value /= max_value;
    }
}

pub fn percentile_in_place(values: &mut [f32], pct: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let len = values.len();
    let target = ((len - 1) as f32 * pct.clamp(0.0, 1.0)).round() as usize;
    let (_, value, _) =
        values.select_nth_unstable_by(target, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    *value
}

pub fn percentile(values: &[f32], pct: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut buf: Vec<f32> = values.to_vec();
    percentile_in_place(&mut buf, pct)
}

pub fn distance_transform(edge_map: &[u8], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(edge_map.len(), width * height);
    let mut dist = vec![f32::MAX; edge_map.len()];
    for (idx, &value) in edge_map.iter().enumerate() {
        if value > 0 {
            dist[idx] = 0.0;
        }
    }

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if dist[idx] == 0.0 {
                continue;
            }
            let mut best = dist[idx];
            if x > 0 {
                best = best.min(dist[idx - 1] + 1.0);
            }
            if y > 0 {
                best = best.min(dist[idx - width] + 1.0);
            }
            if x > 0 && y > 0 {
                best = best.min(dist[idx - width - 1] + SQRT_2);
            }
            if x + 1 < width && y > 0 {
                best = best.min(dist[idx - width + 1] + SQRT_2);
            }
            dist[idx] = best;
        }
    }

    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let idx = y * width + x;
            let mut best = dist[idx];
            if x + 1 < width {
                best = best.min(dist[idx + 1] + 1.0);
            }
            if y + 1 < height {
                best = best.min(dist[idx + width] + 1.0);
            }
            if x + 1 < width && y + 1 < height {
                best = best.min(dist[idx + width + 1] + SQRT_2);
            }
            if x > 0 && y + 1 < height {
                best = best.min(dist[idx + width - 1] + SQRT_2);
            }
            dist[idx] = best;
        }
    }
    dist
}

pub fn dilate_binary(mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
    assert_eq!(mask.len(), width * height);
    let mut current = mask.to_vec();
    let mut next = vec![0u8; mask.len()];
    for _ in 0..iterations {
        for y in 0..height {
            for x in 0..width {
                let mut value = 0u8;
                'outer: for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        if current[ky * width + kx] > 0 {
                            value = 1;
                            break 'outer;
                        }
                    }
                }
                next[y * width + x] = value;
            }
        }
        current.copy_from_slice(&next);
    }
    current
}

pub fn erode_binary(mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
    assert_eq!(mask.len(), width * height);
    let mut current = mask.to_vec();
    let mut next = vec![0u8; mask.len()];
    for _ in 0..iterations {
        for y in 0..height {
            for x in 0..width {
                let mut value = 1u8;
                'outer: for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        if current[ky * width + kx] == 0 {
                            value = 0;
                            break 'outer;
                        }
                    }
                }
                next[y * width + x] = value;
            }
        }
        current.copy_from_slice(&next);
    }
    current
}

pub fn dct2(input: &[f32], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(input.len(), width * height);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut rows = vec![0.0f32; width * height];
    for y in 0..height {
        for u in 0..width {
            let mut sum = 0.0f32;
            for x in 0..width {
                let angle = std::f32::consts::PI / width as f32 * (x as f32 + 0.5) * u as f32;
                sum += input[y * width + x] * angle.cos();
            }
            rows[y * width + u] = sum;
        }
    }
    let mut output = vec![0.0f32; width * height];
    for x in 0..width {
        for v in 0..height {
            let mut sum = 0.0f32;
            for y in 0..height {
                let angle = std::f32::consts::PI / height as f32 * (y as f32 + 0.5) * v as f32;
                sum += rows[y * width + x] * angle.cos();
            }
            output[v * width + x] = sum;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn resize_average_handles_empty_and_downsample() {
        let empty = resize_average(&[], 0, 0, 2, 3);
        assert_eq!(empty, vec![0.0; 6]);

        let pixels = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        ];
        let resized = resize_average(&pixels, 4, 4, 2, 2);
        assert_eq!(resized.len(), 4);
        assert_close(resized[0], 3.5);
        assert_close(resized[1], 5.5);
        assert_close(resized[2], 11.5);
        assert_close(resized[3], 13.5);
    }

    #[test]
    fn gaussian_blur_returns_empty_for_zero_size_and_preserves_uniform_values() {
        assert!(gaussian_blur_3x3(&[], 0, 0).is_empty());

        let input = vec![1.0; 9];
        let output = gaussian_blur_3x3(&input, 3, 3);
        assert_eq!(output.len(), input.len());
        for value in output {
            assert_close(value, 1.0);
        }
    }

    #[test]
    fn sobel_magnitude_clears_output_and_detects_vertical_edge() {
        let mut output = vec![42.0, 42.0];
        sobel_magnitude_into(&[], 0, 0, &mut output);
        assert!(output.is_empty());

        let pixels = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        sobel_magnitude_into(&pixels, 3, 3, &mut output);
        assert_eq!(output.len(), 9);
        assert_close(output[4], 4.0);

        let standalone = sobel_magnitude(&pixels, 3, 3);
        assert_eq!(output, standalone);
    }

    #[test]
    fn normalize_handles_empty_and_zero_max_and_scales_values() {
        let mut empty = Vec::<f32>::new();
        normalize(&mut empty);
        assert!(empty.is_empty());

        let mut near_zero = vec![0.0, f32::EPSILON / 2.0];
        normalize(&mut near_zero);
        assert_eq!(near_zero, vec![0.0, f32::EPSILON / 2.0]);

        let mut values = vec![1.0, 2.0, 4.0];
        normalize(&mut values);
        assert_close(values[0], 0.25);
        assert_close(values[1], 0.5);
        assert_close(values[2], 1.0);
    }

    #[test]
    fn percentile_clamps_bounds_and_supports_copy_variant() {
        let mut low = vec![4.0, 1.0, 9.0, 2.0];
        assert_close(percentile_in_place(&mut low, -1.0), 1.0);

        let mut high = vec![4.0, 1.0, 9.0, 2.0];
        assert_close(percentile_in_place(&mut high, 2.0), 9.0);

        let empty: [f32; 0] = [];
        assert_close(percentile(&empty, 0.5), 0.0);

        let values = vec![4.0, 1.0, 9.0, 2.0];
        assert_close(percentile(&values, 0.5), 4.0);
        assert_eq!(values, vec![4.0, 1.0, 9.0, 2.0]);
    }

    #[test]
    fn distance_transform_produces_expected_distances() {
        let edge_map = vec![0, 0, 0, 0, 1, 0, 0, 0, 0];
        let distances = distance_transform(&edge_map, 3, 3);
        let expected = [SQRT_2, 1.0, SQRT_2, 1.0, 0.0, 1.0, SQRT_2, 1.0, SQRT_2];
        for (actual, expected) in distances.iter().zip(expected) {
            assert_close(*actual, expected);
        }
    }

    #[test]
    fn binary_morphology_respects_iteration_count() {
        let center = vec![0, 0, 0, 0, 1, 0, 0, 0, 0];
        assert_eq!(dilate_binary(&center, 3, 3, 0), center);
        assert_eq!(dilate_binary(&center, 3, 3, 1), vec![1; 9]);

        assert_eq!(erode_binary(&center, 3, 3, 1), vec![0; 9]);
        let full = vec![1; 9];
        assert_eq!(erode_binary(&full, 3, 3, 1), full);
    }

    #[test]
    fn dct2_handles_zero_dimensions_and_constant_input() {
        assert!(dct2(&[], 0, 0).is_empty());

        let input = vec![1.5; 4];
        let output = dct2(&input, 2, 2);
        assert_eq!(output.len(), 4);
        assert_close(output[0], 6.0);
        assert_close(output[1], 0.0);
        assert_close(output[2], 0.0);
        assert_close(output[3], 0.0);
    }
}
