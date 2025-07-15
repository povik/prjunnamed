use std::collections::{BTreeSet, BTreeMap};

use prjunnamed_netlist::{Design, Value, Cell, CellRef, MetaItem, MetaItemRef, Net, ParamValue};
use bumpalo::Bump;
use crate::cst;
use std::borrow::Cow;
use std::cell::RefCell;

#[derive(Default)]
struct Module<'a> {
    name: &'a str,
    submodules: Vec<MetaItemRef<'a>>,

    out_port_nets: BTreeSet<Net>,
    in_port_nets: BTreeSet<Net>,
    present_nets: BTreeSet<Net>,
    cells: Vec<CellRef<'a>>,

    cst: Option<cst::Module<'a>>,
    interface: Option<Value>,
}

use MetaItem::{NamedScope, IndexedScope};

struct HierarchyIndex<'a> {
    flat_regime: bool,
    modules: RefCell<BTreeMap<MetaItemRef<'a>, Module<'a>>>,
    // the containing module for the net's driver
    driver_scopes: BTreeMap<Net, MetaItemRef<'a>>,
    top_scope: MetaItemRef<'a>,
}

fn scope_level(item: MetaItemRef) -> usize {
    if !item.is_none() { scope_level(item.scope_parent().unwrap()) + 1 } else { 0 }
}

fn common_scope<'b>(mut scope1: MetaItemRef<'b>, mut scope2: MetaItemRef<'b>) -> MetaItemRef<'b> {
    let mut level1 = scope_level(scope1);
    let mut level2 = scope_level(scope2);
    while level1 > level2 {
        scope1 = scope1.scope_parent().unwrap();
        level1 -= 1;
    }
    while level2 > level1 {
        scope2 = scope2.scope_parent().unwrap();
        level2 -= 1;
    }
    while scope1 != scope2 {
        scope1 = scope1.scope_parent().unwrap();
        scope2 = scope2.scope_parent().unwrap();
    }
    scope1
}

fn map_metadata<'a>(alloc: &'a Bump, metadata: MetaItemRef) -> cst::Attrs<'a> {
    let mut attrs: Vec<(&'a str, &'a str)> = vec![];
    let mut srcs: Vec<String> = vec![];
    for item in metadata.iter() {
        match item.get() {
            MetaItem::Source { file, start, end } => {
                srcs.push(format!(
                    "{}:{}.{}-{}.{}",
                    file.get(),
                    start.line + 1,
                    start.column + 1,
                    end.line + 1,
                    end.column + 1
                ));
            }
            MetaItem::Attr { name, value } => {
                attrs.push((
                    alloc.alloc(name.get().to_owned()).as_str(),
                    alloc
                        .alloc::<String>(match value.into() {
                            ParamValue::String(str_) => str_,
                            _ => {
                                panic!()
                            }
                        })
                        .as_str(),
                ));
            }
            _ => (),
        }
    }
    if !srcs.is_empty() {
        attrs.push(("src", alloc.alloc(srcs.join("|")).as_str()));
    }
    cst::Attrs(attrs)
}

impl<'a> HierarchyIndex<'a> {
    fn new(top_scope: MetaItemRef<'a>) -> Self {
        Self { flat_regime: true, modules: RefCell::new(BTreeMap::new()), driver_scopes: BTreeMap::new(), top_scope }
    }

    fn find_common_scope<'b>(&self, cell: CellRef<'b>) -> MetaItemRef<'b> {
        if self.flat_regime {
            return MetaItemRef::new_none(cell.design());
        }

        cell.metadata()
            .iter()
            .filter_map(|item_ref| match item_ref.get() {
                NamedScope { .. } | IndexedScope { .. } => Some(item_ref),
                MetaItem::Ident { scope, .. } => Some(scope),
                _ => None,
            })
            .reduce(common_scope)
            .unwrap_or(MetaItemRef::new_none(cell.design()))
    }

    fn find_name_scope<'b>(&self, cell: CellRef<'b>) -> Option<MetaItemRef<'b>> {
        if self.flat_regime {
            return Some(MetaItemRef::new_none(cell.design()));
        }

        let scopes_vec = cell
            .metadata()
            .iter()
            .filter_map(|item_ref| match item_ref.get() {
                NamedScope { .. } | IndexedScope { .. } => Some(item_ref),
                MetaItem::Ident { scope, .. } => Some(scope),
                _ => None,
            })
            .collect::<Vec<MetaItemRef>>();
        if let [scope] = scopes_vec.as_slice() { Some(*scope) } else { None }
    }

    fn initialize_module(alloc: &'a Bump, scope: MetaItemRef<'a>) -> Module<'a> {
        Module {
            name: alloc.alloc(verilog_module_name(scope)).as_str(),
            submodules: vec![],
            out_port_nets: BTreeSet::new(),
            in_port_nets: BTreeSet::new(),
            present_nets: BTreeSet::new(),
            cells: vec![],
            interface: None,
            cst: None,
        }
    }

    fn add_cell(&mut self, alloc: &'a Bump, cell: CellRef<'a>) {
        let mut scope = match &*cell.get() {
            Cell::Name(..) | Cell::Debug(..) => self.find_name_scope(cell).unwrap(),
            Cell::Input(..) | Cell::Output(..) => self.top_scope,
            _ => self.find_common_scope(cell),
        };

        let mut modules = self.modules.borrow_mut();
        let mut new_scope = false;

        if !modules.contains_key(&scope) {
            modules.insert(scope, Self::initialize_module(alloc, scope));
            new_scope = true;
        }

        modules.get_mut(&scope).unwrap().cells.push(cell);
        for net in cell.output() {
            self.driver_scopes.insert(net, scope);
            modules.get_mut(&scope).unwrap().present_nets.insert(net);
        }

        if !new_scope {
            return;
        }

        loop {
            let Some(parent) = scope.scope_parent() else {
                break;
            };

            if modules.contains_key(&parent) {
                modules.get_mut(&parent).unwrap().submodules.push(scope);
                break;
            }

            modules.insert(parent, Self::initialize_module(alloc, parent));
            modules.get_mut(&parent).unwrap().submodules.push(scope);
            scope = parent;
        }
    }

    fn link_net_to_scope(&mut self, net: Net, load: MetaItemRef<'a>) {
        if !net.is_cell() {
            return;
        }

        let driver = *self.driver_scopes.get(&net).unwrap();

        if driver == load {
            return;
        }

        let common = common_scope(driver, load);

        let mut p = driver;
        while p != common {
            self.modules.borrow_mut().get_mut(&p).unwrap().out_port_nets.insert(net);
            self.modules.borrow_mut().get_mut(&p).unwrap().present_nets.insert(net);
            p = p.scope_parent().unwrap();
        }

        self.modules.borrow_mut().get_mut(&p).unwrap().present_nets.insert(net);

        let mut p = load;
        while p != common {
            self.modules.borrow_mut().get_mut(&p).unwrap().in_port_nets.insert(net);
            self.modules.borrow_mut().get_mut(&p).unwrap().present_nets.insert(net);
            p = p.scope_parent().unwrap();
        }
    }

    fn link_cell_inputs(&mut self, cell: CellRef<'a>) {
        if matches!(&*cell.get(), Cell::Name(..) | Cell::Debug(..)) {
            return;
        }

        let load_scope = self.find_common_scope(cell);
        cell.get().visit(|net| {
            self.link_net_to_scope(net, load_scope);
            if net.is_cell() {
                self.modules.borrow_mut().get_mut(&load_scope).unwrap().present_nets.insert(net);
            }
        })
    }

    fn visit_hierarchy(&mut self, scope: MetaItemRef<'a>, f: &mut impl FnMut(&mut Self, MetaItemRef<'a>)) {
        let subscopes = {
            let modules = self.modules.borrow();
            modules.get(&scope).unwrap().submodules.clone()
        };
        for subscope in subscopes {
            self.visit_hierarchy(subscope, f);
        }
        f(self, scope);
    }
}

fn verilog_module_name(scope: MetaItemRef) -> String {
    if scope.is_none() {
        String::from("top")
    } else {
        match scope.get() {
            MetaItem::IndexedScope { index, parent, .. } => {
                format!("{}[{}]", verilog_module_name(parent), index)
            }
            MetaItem::NamedScope { name, parent, .. } => {
                let prefix = verilog_module_name(parent);
                if prefix.is_empty() { String::from(&*name.get()) } else { format!("{}.{}", prefix, name.get()) }
            }
            _ => {
                unreachable!()
            }
        }
    }
}

struct Counter(usize);

impl Counter {
    fn advance(&mut self) -> usize {
        let index = self.0;
        self.0 += 1;
        index
    }
}

fn export_module<'a>(alloc: &'a Bump, index: &mut HierarchyIndex<'a>, scope: MetaItemRef<'a>) {
    let mut port_names: BTreeSet<&'a str> = BTreeSet::new();
    let mut names: BTreeMap<Net, &'a str> = BTreeMap::new();
    let mut modules = index.modules.borrow_mut();
    let module = modules.get(&scope).unwrap();
    let is_top = index.top_scope == scope;

    if is_top {
        for cell_ref in module.cells.iter() {
            if let Cow::Borrowed(cell) = cell_ref.get() {
                match cell {
                    Cell::Input(name, ..) | Cell::Output(name, ..) => {
                        port_names.insert(name);
                    }
                    _ => {}
                }
            }
        }
    }

    for cell_ref in module.cells.iter() {
        if let Cow::Borrowed(cell) = cell_ref.get() {
            match cell {
                Cell::Name(_, value) | Cell::Debug(_, value) => {
                    let Some(name) = cell_ref.metadata().iter().find_map(|item_ref| match item_ref.get() {
                        MetaItem::Ident { name, .. } => Some(alloc.alloc(name.get().to_string()).as_str()),
                        _ => None,
                    }) else {
                        continue;
                    };

                    // ignore mutlibit names for now
                    if value.len() == 1 && !port_names.contains(&name as &str) {
                        names.insert(value.unwrap_net(), &name);
                    }
                }
                _ => {}
            }
        }
    }

    let mut ports: Vec<cst::Member> = vec![];
    let mut wire_decls: Vec<cst::Member> = vec![];
    let mut assigns: Vec<cst::Member> = vec![];
    let mut instantiations: Vec<cst::Member> = vec![];

    for sm_scope in module.submodules.iter() {
        let submodule = modules.get(&sm_scope).unwrap();

        // Submodules should have been processed before this module,
        // so the interface is populated
        let sm_cst = submodule.cst.as_ref().unwrap();
        let sm_interface = submodule.interface.as_ref().unwrap();

        let mut conns: cst::Connections = vec![];
        let mut index: usize = 0;

        for port in sm_cst.port_members.iter() {
            match port {
                cst::Input(_, name, range) | cst::Output(_, name, range) => {
                    // TODO: wider ports
                    assert_eq!(range.len(), 1);
                    conns.push((name, cst::PendingNet(sm_interface[index])));
                    index += range.len();
                }
                _ => {
                    unreachable!();
                }
            }
        }

        instantiations.push(cst::Member::Instantiation(
            cst::Attrs(vec![]),
            alloc.alloc(verilog_module_name(*sm_scope)).as_str(),
            submodule.name,
            conns,
        ));
    }

    // Scout submodule output ports for names
    for sm_scope in module.submodules.iter() {
        let submodule = modules.get(&sm_scope).unwrap();
        let sm_cst = submodule.cst.as_ref().unwrap();
        let sm_interface = submodule.interface.as_ref().unwrap();
        let mut index: usize = 0;
        for port in sm_cst.port_members.iter() {
            match port {
                cst::Output(_, name, range) => {
                    assert_eq!(range.len(), 1);
                    let net = sm_interface[index];
                    if module.present_nets.contains(&net) && !names.contains_key(&net) {
                        let name = alloc.alloc(format!("{}.{}", submodule.name, name)).as_str();
                        names.insert(net, name);
                    }
                    index += range.len();
                }
                cst::Input(_, _, range) => {
                    index += range.len();
                }
                _ => {
                    unreachable!();
                }
            }
        }
    }

    // Scout submodule input ports for names
    for sm_scope in module.submodules.iter() {
        let submodule = modules.get(&sm_scope).unwrap();
        let sm_cst = submodule.cst.as_ref().unwrap();
        let sm_interface = submodule.interface.as_ref().unwrap();
        let mut index: usize = 0;
        for port in sm_cst.port_members.iter() {
            match port {
                cst::Input(_, name, range) => {
                    assert_eq!(range.len(), 1);
                    let net = sm_interface[index];
                    if module.present_nets.contains(&net) && !names.contains_key(&net) {
                        let name = alloc.alloc(format!("{}.{}", submodule.name, name)).as_str();
                        names.insert(net, name);
                    }
                    index += range.len();
                }
                cst::Output(_, _, range) => {
                    index += range.len();
                }
                _ => {
                    unreachable!();
                }
            }
        }
    }

    // Assign missing names
    let mut next_name: Counter = Counter(0);
    for net in module.present_nets.iter() {
        if names.contains_key(&net) {
            continue;
        }

        let name = loop {
            let index = next_name.advance();
            let candidate = alloc.alloc(format!("_{:05}_", index)).as_str();
            if !port_names.contains(&candidate) {
                break candidate;
            }
        };

        names.insert(*net, name);
    }

    // Visit cells
    for cell_ref in module.cells.iter() {
        if let Cow::Borrowed(cell) = cell_ref.get() {
            match cell {
                Cell::Name(..) | Cell::Debug(..) => {}
                Cell::Other(instance) => {
                    if !instance.ios.is_empty() || !instance.params.is_empty() {
                        unimplemented!();
                    }

                    let name = loop {
                        let index = next_name.advance();
                        let candidate = alloc.alloc(format!("_{:05}_", index)).as_str();
                        if !port_names.contains(&candidate) {
                            break candidate;
                        }
                    };

                    let mut conns: cst::Connections = vec![];
                    for (name, value) in instance.inputs.iter() {
                        conns.push((
                            name.as_str(),
                            cst::Concat(value.iter().map(|net| cst::PendingNet(net)).collect::<Vec<_>>()),
                        ));
                    }
                    for (name, range) in instance.outputs.iter() {
                        conns.push((
                            name.as_str(),
                            cst::Concat(
                                range.clone().map(|idx| cst::PendingNet(cell_ref.output()[idx])).collect::<Vec<_>>(),
                            ),
                        ));
                    }
                    instantiations.push(cst::Member::Instantiation(
                        map_metadata(alloc, cell_ref.metadata()),
                        &instance.kind,
                        &name,
                        conns,
                    ));
                }
                Cell::Input(..) | Cell::Output(..) => {}
                _ => {
                    unimplemented!("{}", cell_ref.design().display_cell(*cell_ref))
                }
            }
        }
    }

    for net in module.present_nets.iter() {
        if !module.in_port_nets.contains(net) && !module.out_port_nets.contains(net) {
            wire_decls.push(cst::Wire(cst::Attrs(vec![]), names.get(&net).unwrap(), cst::Range { msb: 0, lsb: 0 }))
        }
    }

    let mut interface = Value::new();

    if is_top {
        for cell_ref in module.cells.iter() {
            if let Cow::Borrowed(cell) = cell_ref.get() {
                match cell {
                    Cell::Input(name, size) => {
                        ports.push(cst::Input(cst::Attrs(vec![]), name, cst::Range { msb: size - 1, lsb: 0 }));
                        let output = cell_ref.output();
                        assigns.push(cst::Assign(cst::Expression::from_value(&output), cst::NamedValue(name)));
                    }
                    Cell::Output(name, value) => {
                        ports.push(cst::Output(cst::Attrs(vec![]), name, cst::Range { msb: value.len() - 1, lsb: 0 }));
                        assigns.push(cst::Assign(cst::NamedValue(name), cst::Expression::from_value(value)));
                    }
                    _ => {}
                }
            }
        }
    } else {
        for net in module.in_port_nets.iter() {
            let name = names.get(&net).unwrap();
            interface.push(net);
            ports.push(cst::Input(cst::Attrs(vec![]), name, cst::Range { lsb: 0, msb: 0 }));
        }

        for net in module.out_port_nets.iter() {
            let name = names.get(&net).unwrap();
            interface.push(net);
            ports.push(cst::Output(cst::Attrs(vec![]), name, cst::Range { lsb: 0, msb: 0 }));
        }
    }

    let mut members = Vec::new();
    members.append(&mut wire_decls);
    members.append(&mut assigns);
    members.append(&mut instantiations);
    for member in members.iter_mut() {
        member.visit_expressions(&mut |expr| match expr {
            cst::PendingNet(net) => {
                *expr = cst::NamedValue(names.get(&net).unwrap());
            }
            _ => {}
        });
    }

    let module = modules.get_mut(&scope).unwrap();
    if !is_top {
        module.interface = Some(interface);
    }
    module.cst = Some(cst::Module { name: module.name, port_members: ports, members });
}

pub fn export<'a>(design: &'a Design, alloc: &'a Bump) -> cst::Unit<'a> {
    // FIXME: this picks the uppermost scope with cells (which are not
    // input/output cells) as the top; the proper logic here looks to be
    // to pick the one scope with parent=none
    let top_scope = design
        .iter_cells()
        .filter(|cell| !matches!(&*cell.get(), Cell::Input(..) | Cell::Output(..)))
        .map(|cell| {
            cell.metadata()
                .iter()
                .filter_map(|item_ref| match item_ref.get() {
                    NamedScope { .. } | IndexedScope { .. } => Some(item_ref),
                    MetaItem::Ident { scope, .. } => Some(scope),
                    _ => None,
                })
                .reduce(common_scope)
                .unwrap_or(MetaItemRef::new_none(design))
        })
        .reduce(common_scope)
        .unwrap_or(MetaItemRef::new_none(design));

    let mut index = HierarchyIndex::new(top_scope);

    for cell in design.iter_cells() {
        index.add_cell(alloc, cell);
    }
    for cell in design.iter_cells() {
        index.link_cell_inputs(cell);
    }

    index.visit_hierarchy(top_scope, &mut |index, scope| {
        export_module(&alloc, index, scope);
    });

    let mut cst_modules: Vec<cst::Module> = vec![];
    index.visit_hierarchy(top_scope, &mut |index, scope| {
        let mut modules = index.modules.borrow_mut();
        let module = modules.get_mut(&scope).unwrap();
        cst_modules.push(std::mem::take(&mut module.cst).unwrap());
    });
    cst::Unit(cst_modules)
}
