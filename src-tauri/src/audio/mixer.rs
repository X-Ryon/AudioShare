//! 混音引擎核心（技术设计§4 mixer.rs）
//! 追溯：TC-002（每源独立音量）、TC-003（断流剔除）、稳定性设计（饱和限幅）

/// 多源混音：每源乘增益后求和，输出饱和限幅至 [-1.0, 1.0]。
/// 缓冲区长度不一致时按最长补零。
pub fn mix(buffers: &[(&[f32], f32)]) -> Vec<f32> {
    let len = buffers.iter().map(|(b, _)| b.len()).max().unwrap_or(0);
    let mut out = vec![0f32; len];
    for (buf, gain) in buffers {
        for (i, s) in buf.iter().enumerate() {
            let v = out[i] + s * gain;
            out[i] = v.clamp(-1.0, 1.0);
        }
    }
    out
}

/// 计算 PCM 帧的 RMS 电平（用于前端电平条）。
pub fn rms_level(frames: &[f32]) -> f32 {
    if frames.is_empty() {
        return 0.0;
    }
    let sum: f32 = frames.iter().map(|s| s * s).sum();
    (sum / frames.len() as f32).sqrt()
}

/// 源增益表：管理每个共享源（按 PID）的音量增益。
#[derive(Debug, Default)]
pub struct SourceTable {
    gains: std::collections::HashMap<u32, f32>,
}

impl SourceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, pid: u32) {
        self.gains.entry(pid).or_insert(1.0);
    }

    pub fn remove(&mut self, pid: u32) {
        self.gains.remove(&pid);
    }

    pub fn set_gain(&mut self, pid: u32, gain: f32) {
        if self.gains.contains_key(&pid) {
            self.gains.insert(pid, gain.clamp(0.0, 1.0));
        }
    }

    pub fn gain(&self, pid: u32) -> Option<f32> {
        self.gains.get(&pid).copied()
    }

    pub fn pids(&self) -> Vec<u32> {
        self.gains.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc002_gain_zero_mutes_source() {
        // 音量滑杆至0 → 该源静音
        let out = mix(&[(&[0.5, -0.5, 0.25], 0.0)]);
        assert_eq!(out, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn tc002_gain_scales_amplitude() {
        // 音量0.5 → 幅度减半
        let out = mix(&[(&[0.8, -0.6], 0.5)]);
        assert!((out[0] - 0.4).abs() < 1e-6);
        assert!((out[1] + 0.3).abs() < 1e-6);
    }

    #[test]
    fn tc001_multiple_sources_summed() {
        // 多源求和：应用 + 人声
        let out = mix(&[(&[0.5, 0.5], 1.0), (&[0.25, -0.25], 1.0)]);
        assert!((out[0] - 0.75).abs() < 1e-6);
        assert!((out[1] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn stability_saturation_clamps_to_pm1() {
        // 饱和限幅：0.8+0.8=1.6 → 1.0，防爆音
        let out = mix(&[(&[0.8], 1.0), (&[0.8], 1.0)]);
        assert_eq!(out[0], 1.0);
        let out2 = mix(&[(&[-0.9], 1.0), (&[-0.9], 1.0)]);
        assert_eq!(out2[0], -1.0);
    }

    #[test]
    fn stability_empty_input_returns_empty() {
        assert!(mix(&[]).is_empty());
    }

    #[test]
    fn stability_varying_lengths_padded() {
        // 源长度不一致按最长补零（麦克风与应用帧不同步）
        let out = mix(&[(&[0.5, 0.5, 0.5], 1.0), (&[0.1], 1.0)]);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 0.6).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn tc003_rms_level_calculation() {
        // 电平计算：RMS
        let rms = rms_level(&[0.5, -0.5, 0.5, -0.5]);
        assert!((rms - 0.5).abs() < 1e-6);
        assert_eq!(rms_level(&[0.0; 4]), 0.0);
    }

    #[test]
    fn tc003_source_table_manage() {
        // 源表：增/删/改增益；剔除不存在的源不报错
        let mut t = SourceTable::new();
        t.add(100);
        t.add(200);
        t.set_gain(100, 0.25);
        assert_eq!(t.gain(100), Some(0.25));
        assert_eq!(t.gain(200), Some(1.0)); // 默认满增益
        assert_eq!(t.gain(999), None);
        t.remove(100);
        assert_eq!(t.gain(100), None);
        t.remove(100); // 幂等
    }
}
