//! Resolve explicit focus in custom editors whose application AX focus is absent.
//! Only roles, focus flags, children and window identity are inspected here.
use super::*;
use core_foundation::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex};
use std::collections::VecDeque;

// QQ's raw tree includes wrapper nodes omitted by compact accessibility tools.
// Keep a separate node bound and wall-clock budget; never accept partial scans.
const MAX_NODES: usize = 1_024;
const MAX_DEPTH: usize = 32;
const SEARCH_BUDGET: Duration = Duration::from_millis(200);

trait Tree {
    type Node;
    fn same(&self, left: &Self::Node, right: &Self::Node) -> bool;
    fn focused(&self, node: &Self::Node) -> Result<bool, Failure>;
    fn children(&self, node: &Self::Node) -> Result<Vec<Self::Node>, Failure>;
    fn within_budget(&self) -> bool;
}

/// A complete bounded scan is required: choosing the first editable field or
/// the first focused flag could adopt a different field in a partial AX tree.
fn search<T: Tree>(tree: &T, root: T::Node) -> Result<Option<T::Node>, Failure> {
    let mut pending = VecDeque::from([(root, 0)]);
    let mut visited = Vec::new();
    let mut focused_index = None;
    while let Some((node, depth)) = pending.pop_front() {
        if !tree.within_budget() {
            return Err(Failure::Focus);
        }
        if visited.iter().any(|other| tree.same(&node, other)) {
            continue;
        }
        if visited.len() >= MAX_NODES {
            return Err(Failure::Focus);
        }
        // The focused window itself is not the text receiver. Its descendants
        // must explicitly identify focus, independently of their editable role.
        if depth > 0 && tree.focused(&node)? {
            if focused_index.is_some() {
                return Err(Failure::Focus);
            }
            focused_index = Some(visited.len());
        }
        let children = tree.children(&node)?;
        if (depth >= MAX_DEPTH && !children.is_empty())
            || pending.len() + children.len() > MAX_NODES
        {
            return Err(Failure::Focus);
        }
        pending.extend(children.into_iter().map(|child| (child, depth + 1)));
        visited.push(node);
    }
    match focused_index {
        Some(index) if tree.within_budget() && tree.focused(&visited[index])? => {
            Ok(Some(visited.swap_remove(index)))
        }
        Some(_) => Err(Failure::Focus),
        None => Ok(None),
    }
}

struct NativeTree {
    started: Instant,
}
impl NativeTree {
    fn attribute(&self, element: CFTypeRef, name: &str) -> Result<OwnedCf, i32> {
        const CANNOT_COMPLETE: i32 = -25204;
        let remaining = SEARCH_BUDGET.saturating_sub(self.started.elapsed());
        if remaining.is_zero() {
            return Err(CANNOT_COMPLETE);
        }
        // AX timeouts belong to an object, not its process or equal objects.
        // Configure every receiver, including the application/window lookups.
        let timeout = remaining.min(Duration::from_millis(50)).as_secs_f32();
        if unsafe { AXUIElementSetMessagingTimeout(element, timeout) } != 0 {
            return Err(CANNOT_COMPLETE);
        }
        let result = attribute(element, name);
        if !self.within_budget() {
            return Err(CANNOT_COMPLETE);
        }
        result
    }
    fn window(&self, application: &OwnedCf) -> Result<OwnedCf, Failure> {
        let window = self
            .attribute(application.0, "AXFocusedWindow")
            .map_err(Failure::FocusRead)?;
        if unsafe { CFGetTypeID(window.0) != AXUIElementGetTypeID() } {
            return Err(Failure::Focus);
        }
        Ok(window)
    }
}
impl Tree for NativeTree {
    type Node = OwnedCf;
    fn same(&self, left: &OwnedCf, right: &OwnedCf) -> bool {
        unsafe { CFEqual(left.0, right.0) != 0 }
    }
    fn focused(&self, node: &OwnedCf) -> Result<bool, Failure> {
        match self.attribute(node.0, "AXFocused") {
            Ok(value) if unsafe { CFGetTypeID(value.0) == CFBooleanGetTypeID() } => {
                Ok(unsafe { CFEqual(value.0, kCFBooleanTrue.cast()) != 0 })
            }
            Err(AX_UNSUPPORTED | AX_NO_VALUE) => Ok(false),
            _ => Err(Failure::Focus),
        }
    }
    fn children(&self, node: &OwnedCf) -> Result<Vec<OwnedCf>, Failure> {
        let children = match self.attribute(node.0, "AXChildren") {
            Ok(children) => children,
            Err(AX_UNSUPPORTED | AX_NO_VALUE) => return Ok(Vec::new()),
            Err(_) => return Err(Failure::Focus),
        };
        if unsafe { CFGetTypeID(children.0) != CFArrayGetTypeID() } {
            return Err(Failure::Focus);
        }
        let count = unsafe { CFArrayGetCount(children.0.cast()) };
        if count < 0 || count as usize > MAX_NODES {
            return Err(Failure::Focus);
        }
        (0..count)
            .map(|index| {
                let value = unsafe { CFArrayGetValueAtIndex(children.0.cast(), index) };
                if value.is_null() || unsafe { CFGetTypeID(value) != AXUIElementGetTypeID() } {
                    return Err(Failure::Focus);
                }
                // Array entries are borrowed. Retain before the array drops.
                unsafe { core_foundation::base::CFRetain(value) };
                OwnedCf::new(value).ok_or(Failure::Focus)
            })
            .collect()
    }
    fn within_budget(&self) -> bool {
        self.started.elapsed() < SEARCH_BUDGET
    }
}

fn missing_application_focus(status: i32) -> bool {
    matches!(status, AX_NO_VALUE | AX_UNSUPPORTED | -25208)
}

pub(super) struct Resolved {
    pub element: OwnedCf,
    pub source: &'static str,
}

pub(super) fn resolve(pid: libc::pid_t) -> Result<Resolved, Failure> {
    let tree = NativeTree {
        started: Instant::now(),
    };
    let application =
        OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) }).ok_or(Failure::Focus)?;
    let missing = match tree.attribute(application.0, "AXFocusedUIElement") {
        Ok(element) => {
            if unsafe { CFGetTypeID(element.0) != AXUIElementGetTypeID() } {
                return Err(Failure::Focus);
            }
            return Ok(Resolved {
                element,
                source: "application",
            });
        }
        Err(status) if missing_application_focus(status) => status,
        Err(status) => return Err(Failure::FocusRead(status)),
    };
    let original = tree.window(&application)?;
    unsafe { core_foundation::base::CFRetain(original.0) };
    let window = OwnedCf::new(original.0).ok_or(Failure::Focus)?;
    let found = search(&tree, window)?;
    let current = tree.window(&application)?;
    if unsafe { CFEqual(original.0, current.0) == 0 } {
        return Err(Failure::FocusElementChanged);
    }
    let element = found.ok_or(Failure::FocusRead(missing))?;
    Ok(Resolved {
        element,
        source: "window_focused_descendant",
    })
}

/// AX positions use screen points with a top-left origin. Missing geometry is
/// diagnostic null, never a reason to guess a field or reject valid delivery.
pub(super) fn bounds(element: &OwnedCf) -> serde_json::Value {
    use core_graphics::geometry::{CGPoint, CGSize};
    let tree = NativeTree {
        started: Instant::now(),
    };
    let (Ok(position), Ok(size)) = (
        tree.attribute(element.0, "AXPosition"),
        tree.attribute(element.0, "AXSize"),
    ) else {
        return serde_json::Value::Null;
    };
    let mut point = CGPoint::new(0.0, 0.0);
    let mut dimensions = CGSize::new(0.0, 0.0);
    let valid = unsafe {
        CFGetTypeID(position.0) == AXValueGetTypeID()
            && CFGetTypeID(size.0) == AXValueGetTypeID()
            && AXValueGetType(position.0) == 1
            && AXValueGetType(size.0) == 2
            && AXValueGetValue(position.0, 1, (&mut point as *mut CGPoint).cast())
            && AXValueGetValue(size.0, 2, (&mut dimensions as *mut CGSize).cast())
    };
    if !valid {
        return serde_json::Value::Null;
    }
    geometry(point.x, point.y, dimensions.width, dimensions.height)
}

fn geometry(x: f64, y: f64, width: f64, height: f64) -> serde_json::Value {
    if ![x, y, width, height].into_iter().all(f64::is_finite) || width <= 0.0 || height <= 0.0 {
        return serde_json::Value::Null;
    }
    serde_json::json!({
        "x": x, "y": y, "width": width, "height": height,
        "coordinateSpace": "screen_points_top_left",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fixture {
        children: Vec<Vec<usize>>,
        focused: Vec<usize>,
        fail_at: Option<usize>,
        budget: bool,
        focus_reads: Cell<usize>,
        blur_on_recheck: bool,
    }
    impl Tree for Fixture {
        type Node = usize;
        fn same(&self, a: &usize, b: &usize) -> bool {
            a == b
        }
        fn focused(&self, node: &usize) -> Result<bool, Failure> {
            let reads = self.focus_reads.get();
            self.focus_reads.set(reads + 1);
            if self.fail_at == Some(*node) {
                Err(Failure::Focus)
            } else {
                Ok(self.focused.contains(node)
                    && !(self.blur_on_recheck && reads >= self.children.len() - 1))
            }
        }
        fn children(&self, node: &usize) -> Result<Vec<usize>, Failure> {
            Ok(self.children[*node].clone())
        }
        fn within_budget(&self) -> bool {
            self.budget
        }
    }
    fn fixture(children: Vec<Vec<usize>>, focused: Vec<usize>) -> Fixture {
        Fixture {
            children,
            focused,
            fail_at: None,
            budget: true,
            focus_reads: Cell::new(0),
            blur_on_recheck: false,
        }
    }
    #[test]
    fn qq_missing_application_focus_uses_only_explicit_descendant_focus() {
        // Search field comes before composer; an editable role alone is no proof.
        let tree = fixture(vec![vec![1, 2], vec![], vec![]], vec![2]);
        assert_eq!(search(&tree, 0), Ok(Some(2)));
        let tree = fixture(vec![vec![1, 2], vec![], vec![]], vec![]);
        assert_eq!(search(&tree, 0), Ok(None));
    }
    #[test]
    fn window_focus_is_not_a_text_receiver_and_multiple_flags_fail_closed() {
        let tree = fixture(vec![vec![1], vec![]], vec![0]);
        assert_eq!(search(&tree, 0), Ok(None));
        let tree = fixture(vec![vec![1, 2], vec![], vec![]], vec![1, 2]);
        assert_eq!(search(&tree, 0), Err(Failure::Focus));
    }
    #[test]
    fn shared_nodes_and_cycles_do_not_duplicate_or_recurse_forever() {
        let tree = fixture(vec![vec![1, 2], vec![2], vec![0]], vec![2]);
        assert_eq!(search(&tree, 0), Ok(Some(2)));
    }
    #[test]
    fn partial_trees_errors_expired_budget_and_lost_focus_are_rejected() {
        let mut tree = fixture(vec![vec![1, 2], vec![], vec![]], vec![1]);
        tree.fail_at = Some(2);
        assert_eq!(search(&tree, 0), Err(Failure::Focus));
        tree.fail_at = None;
        tree.budget = false;
        assert_eq!(search(&tree, 0), Err(Failure::Focus));
        tree.budget = true;
        tree.focus_reads.set(0);
        tree.blur_on_recheck = true;
        assert_eq!(search(&tree, 0), Err(Failure::Focus));
    }
    #[test]
    fn complete_qq_sized_tree_and_exact_node_limit_are_supported() {
        // The observed QQ raw tree contains 462 nodes, including native wrappers.
        for count in [462, MAX_NODES] {
            let mut children = vec![vec![]; count];
            children[0] = (1..count).collect();
            let tree = fixture(children, vec![count - 1]);
            assert_eq!(search(&tree, 0), Ok(Some(count - 1)));
        }
    }
    #[test]
    fn wide_and_deep_partial_scans_never_accept_an_early_candidate() {
        let mut wide = vec![vec![]; MAX_NODES + 2];
        wide[0] = (1..wide.len()).collect();
        assert_eq!(search(&fixture(wide, vec![1]), 0), Err(Failure::Focus));
        let mut deep = vec![vec![]; MAX_DEPTH + 3];
        for i in 0..deep.len() - 1 {
            deep[i].push(i + 1);
        }
        assert_eq!(search(&fixture(deep, vec![1]), 0), Err(Failure::Focus));
    }
    #[test]
    fn geometry_keeps_negative_monitor_origins_and_rejects_invalid_frames() {
        let frame = geometry(-600.0, -100.0, 572.0, 119.0);
        assert_eq!(frame["x"], -600.0);
        assert_eq!(frame["coordinateSpace"], "screen_points_top_left");
        for frame in [
            geometry(0.0, 0.0, 0.0, 119.0),
            geometry(0.0, 0.0, 572.0, -1.0),
            geometry(f64::NAN, 0.0, 572.0, 119.0),
            geometry(0.0, f64::INFINITY, 572.0, 119.0),
        ] {
            assert!(frame.is_null());
        }
    }
    #[test]
    fn expired_native_budget_returns_before_sending_an_ax_message() {
        let tree = NativeTree {
            started: Instant::now() - SEARCH_BUDGET,
        };
        assert!(matches!(
            tree.attribute(ptr::null(), "AXFocusedUIElement"),
            Err(-25204)
        ));
    }
    #[test]
    fn permission_and_transport_errors_never_trigger_tree_search() {
        for status in [-25204, -25211, -25202, 0] {
            assert!(!missing_application_focus(status));
        }
        for status in [AX_NO_VALUE, AX_UNSUPPORTED, -25208] {
            assert!(missing_application_focus(status));
        }
    }
}
