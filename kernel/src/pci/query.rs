
use super::resources::{self, PciResource};
use super::PciBarKind;

// ============================================================
// QUERY DEFINITION
// ============================================================

#[derive(Clone, Copy)]
pub struct ResourceQuery {
    pub bus: Option<u8>,
    pub device: Option<u8>,
    pub function: Option<u8>,

    pub vendor_id: Option<u16>,
    pub device_id: Option<u16>,

    pub bar_index: Option<u8>,
    pub kind: Option<PciBarKind>,

    pub range_start: Option<u64>,
    pub range_end: Option<u64>,

    pub known_size_only: bool,
}

impl ResourceQuery {
    pub const fn all() -> Self {
        Self {
            bus: None,
            device: None,
            function: None,
            vendor_id: None,
            device_id: None,
            bar_index: None,
            kind: None,
            range_start: None,
            range_end: None,
            known_size_only: false,
        }
    }

    pub const fn by_address(
        bus: u8,
        device: u8,
        function: u8,
    ) -> Self {
        let mut query = Self::all();

        query.bus = Some(bus);
        query.device = Some(device);
        query.function = Some(function);

        query
    }

    pub const fn by_ids(
        vendor_id: u16,
        device_id: u16,
    ) -> Self {
        let mut query = Self::all();

        query.vendor_id = Some(vendor_id);
        query.device_id = Some(device_id);

        query
    }

    pub const fn by_bar(index: u8) -> Self {
        let mut query = Self::all();
        query.bar_index = Some(index);
        query
    }

    pub const fn with_kind(
        mut self,
        kind: PciBarKind,
    ) -> Self {
        self.kind = Some(kind);
        self
    }

    pub const fn known_sizes_only(
        mut self,
    ) -> Self {
        self.known_size_only = true;
        self
    }

    pub const fn in_range(
        mut self,
        start: u64,
        end: u64,
    ) -> Self {
        self.range_start = Some(start);
        self.range_end = Some(end);
        self
    }

    pub const fn is_valid(&self) -> bool {
        match (self.range_start, self.range_end) {
            (Some(start), Some(end)) => start <= end,
            (Some(_), None) => false,
            (None, Some(_)) => false,
            (None, None) => true,
        }
    }

    fn matches(&self, resource: &PciResource) -> bool {
        if !resource.bar.valid {
            return false;
        }

        if let Some(value) = self.bus {
            if resource.bus != value {
                return false;
            }
        }

        if let Some(value) = self.device {
            if resource.device != value {
                return false;
            }
        }

        if let Some(value) = self.function {
            if resource.function != value {
                return false;
            }
        }

        if let Some(value) = self.vendor_id {
            if resource.vendor_id != value {
                return false;
            }
        }

        if let Some(value) = self.device_id {
            if resource.device_id != value {
                return false;
            }
        }

        if let Some(value) = self.bar_index {
            if resource.bar.index != value {
                return false;
            }
        }

        if let Some(value) = self.kind {
            if resource.bar.kind != value {
                return false;
            }
        }

        if self.known_size_only && !resource.bar.size_known {
            return false;
        }

        // If an address range was requested, the resource must
        // have a known and non-overflowing end address.
        if let (Some(start), Some(end)) =
            (self.range_start, self.range_end)
        {
            let resource_end = match resource.end_address() {
                Some(value) => value,
                None => return false,
            };

            if resource.bar.base > end || resource_end < start {
                return false;
            }
        }

        true
    }
}

// ============================================================
// QUERY RESULT
// ============================================================

#[derive(Clone, Copy)]
pub struct QueryResult {
    pub found: usize,
    pub truncated: bool,
}

impl QueryResult {
    pub const EMPTY: Self = Self {
        found: 0,
        truncated: false,
    };
}

// ============================================================
// QUERY EXECUTION
// ============================================================

/// Executes a resource query and writes matching resources
/// into the caller-provided output buffer.
///
/// Returns the number of matching resources copied.
pub fn execute(
    query: &ResourceQuery,
    output: &mut [PciResource],
) -> QueryResult {
    if !query.is_valid() {
        return QueryResult::EMPTY;
    }

    let total = resources::count();

    let mut result = QueryResult::EMPTY;

    for index in 0..total {
        let resource = match resources::get(index) {
            Some(resource) => resource,
            None => continue,
        };

        if !query.matches(&resource) {
            continue;
        }

        result.found += 1;

        if result.found <= output.len() {
            output[result.found - 1] = resource;
        } else {
            result.truncated = true;
        }
    }

    result
}

// ============================================================
// COUNT MATCHING RESOURCES
// ============================================================

pub fn count_matching(query: &ResourceQuery) -> usize {
    if !query.is_valid() {
        return 0;
    }

    let total = resources::count();
    let mut found = 0;

    for index in 0..total {
        let resource = match resources::get(index) {
            Some(resource) => resource,
            None => continue,
        };

        if query.matches(&resource) {
            found += 1;
        }
    }

    found
}

// ============================================================
// FIND FIRST MATCH
// ============================================================

pub fn find_first(
    query: &ResourceQuery,
) -> Option<PciResource> {
    if !query.is_valid() {
        return None;
    }

    let total = resources::count();

    for index in 0..total {
        let resource = resources::get(index)?;

        if query.matches(&resource) {
            return Some(resource);
        }
    }

    None
}

// ============================================================
// CONVENIENCE FUNCTIONS
// ============================================================

pub fn find_device_resources(
    bus: u8,
    device: u8,
    function: u8,
    output: &mut [PciResource],
) -> QueryResult {
    let query = ResourceQuery::by_address(
        bus,
        device,
        function,
    );

    execute(&query, output)
}

pub fn find_memory_resources(
    output: &mut [PciResource],
) -> QueryResult {
    let kinds = [
        PciBarKind::Memory32,
        PciBarKind::Memory64,
        PciBarKind::MemoryBelow1M,
    ];

    let mut total_found = 0usize;
    let mut written = 0usize;

    for kind in kinds {
        let query = ResourceQuery::all().with_kind(kind);
        let remaining = output.len().saturating_sub(written);

        let result = execute(
            &query,
            &mut output[written..written + remaining],
        );

        written += result.found.min(remaining);
        total_found += result.found;
    }

    QueryResult {
        found: total_found,
        truncated: total_found > output.len(),
    }
}

pub fn find_io_resources(
    output: &mut [PciResource],
) -> QueryResult {
    let query = ResourceQuery::all()
        .with_kind(PciBarKind::Io);

    execute(&query, output)
}
