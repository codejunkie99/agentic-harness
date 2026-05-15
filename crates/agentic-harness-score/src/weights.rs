#[derive(Debug, Clone, Copy)]
pub struct ScoreWeights {
    pub completion: f64,
    pub efficiency: f64,
    pub tool_success: f64,
    pub recovery: f64,
    pub diff_quality: f64,
    pub planning_quality: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            completion: 0.30,
            efficiency: 0.18,
            tool_success: 0.15,
            recovery: 0.15,
            diff_quality: 0.12,
            planning_quality: 0.10,
        }
    }
}
