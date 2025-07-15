use std::{cell::RefCell, collections::HashMap};

use prjunnamed_netlist::{Cell, Design, Net, RewriteRuleset, Value};

pub struct LevelAnalysis {
    levels: RefCell<HashMap<Net, u32>>,
}

impl LevelAnalysis {
    pub fn new() -> Self {
        LevelAnalysis { levels: Default::default() }
    }

    pub fn get(&self, net: Net) -> u32 {
        self.levels.borrow().get(&net).copied().unwrap_or(0)
    }
}

impl RewriteRuleset for LevelAnalysis {
    fn cell_added(&self, design: &Design, cell: &Cell, output: &Value) {
        let mut levels = self.levels.borrow_mut();
        if let Cell::Not(input) = cell {
            for (onet, inet) in output.iter().zip(input) {
                let ilevel = levels.get(&inet).copied().unwrap_or(0);
                levels.insert(onet, ilevel);
            }
        }
        if !cell.has_state(design) {
            let fudge: u32 = match cell {
                Cell::Adc(arg1, _, _) => 2 * ((arg1.len() as f32).log2().ceil() + 1.0) as u32,
                Cell::Mul(arg1, arg2) => (arg1.len() + arg2.len()) as u32,
                _ => 1,
            };

            let mut level = 0;
            cell.visit(|net| {
                if !net.is_const() {
                    let l = levels.get(&net).copied().unwrap_or(0);
                    level = level.max(l + fudge);
                }
            });
            for net in output {
                levels.insert(net, level);
            }
        };
    }

    fn net_replaced(&self, _design: &Design, from: Net, to: Net) {
        let mut levels = self.levels.borrow_mut();
        if let Some(&level) = levels.get(&to) {
            levels.insert(from, level);
        }
    }
}
