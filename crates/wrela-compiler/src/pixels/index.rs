//! Immutable offset/count indexes for local renderer retrieval.

use super::ids::{DerivativeBundleId, FeatureId, MaterialId, ObjectId};
use super::projection_bounds::{ProjectedFeatureSpan, TILE_HEIGHT_V1, TILE_WIDTH_V1};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexSlice {
    pub offset: u32,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompressedIndex {
    pub cells: Vec<IndexSlice>,
    pub ids: Vec<u32>,
}

impl CompressedIndex {
    /// Runtime shape: one checked cell load, checked offset+count, then one
    /// contiguous slice.
    pub fn lookup(&self, cell: u32) -> Result<&[u32], String> {
        let cell_index = usize::try_from(cell)
            .map_err(|_| "pixels::index: cell ID exceeds usize".to_string())?;
        let slice = self.cells.get(cell_index).ok_or_else(|| {
            format!(
                "pixels::index: cell {cell} is outside {} cells",
                self.cells.len()
            )
        })?;
        let start = usize::try_from(slice.offset)
            .map_err(|_| "pixels::index: slice offset exceeds usize".to_string())?;
        let end = slice
            .offset
            .checked_add(slice.count)
            .and_then(|end| usize::try_from(end).ok())
            .ok_or_else(|| "pixels::index: slice end overflow".to_string())?;
        self.ids
            .get(start..end)
            .ok_or_else(|| format!("pixels::index: cell {cell} slice is out of bounds"))
    }

    pub fn encoded_bytes(&self) -> Result<u64, String> {
        let cells = u64::try_from(self.cells.len())
            .map_err(|_| "P015: index cell byte count overflow".to_string())?
            .checked_mul(8)
            .ok_or_else(|| "P015: index cell byte count overflow".to_string())?;
        let ids = u64::try_from(self.ids.len())
            .map_err(|_| "P015: index ID byte count overflow".to_string())?
            .checked_mul(4)
            .ok_or_else(|| "P015: index ID byte count overflow".to_string())?;
        cells
            .checked_add(ids)
            .ok_or_else(|| "P015: index byte count overflow".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectFeatureRange {
    pub object: ObjectId,
    pub first: FeatureId,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialProgramIndex {
    pub identity_set: u32,
    pub material: MaterialId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalIndexes {
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub tile_features: CompressedIndex,
    pub tile_events: CompressedIndex,
    pub tile_competitions: CompressedIndex,
    pub row_block_repeats: CompressedIndex,
    pub object_features: Vec<ObjectFeatureRange>,
    pub feature_derivatives: Vec<DerivativeBundleId>,
    pub material_programs: Vec<MaterialProgramIndex>,
    pub tile_lights: CompressedIndex,
    pub tile_probes: CompressedIndex,
    pub bytes: u64,
}

fn compress(cells: Vec<Vec<u32>>, kind: &str) -> Result<CompressedIndex, String> {
    let mut slices = Vec::with_capacity(cells.len());
    let mut ids = Vec::new();
    for (cell, mut values) in cells.into_iter().enumerate() {
        values.sort_unstable();
        if values.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(format!("pixels::index: duplicate {kind} ID in cell {cell}"));
        }
        let offset =
            u32::try_from(ids.len()).map_err(|_| format!("P015: {kind} index offset overflow"))?;
        let count = u32::try_from(values.len())
            .map_err(|_| format!("P015: {kind} index slice count overflow"))?;
        ids.extend(values);
        slices.push(IndexSlice { offset, count });
    }
    Ok(CompressedIndex { cells: slices, ids })
}

fn tile_cell(x: u32, y: u32, tiles_x: u32) -> Result<usize, String> {
    let cell = y
        .checked_mul(tiles_x)
        .and_then(|value| value.checked_add(x))
        .ok_or_else(|| "P015: tile cell index overflow".to_string())?;
    usize::try_from(cell).map_err(|_| "P015: tile cell index exceeds usize".to_string())
}

fn add_tile_span(
    cells: &mut [Vec<u32>],
    span: super::projection_bounds::TileSpan,
    tiles_x: u32,
    id: u32,
) -> Result<(), String> {
    for y in span.y.start..span.y.end {
        for x in span.x.start..span.x.end {
            let cell = tile_cell(x, y, tiles_x)?;
            cells
                .get_mut(cell)
                .ok_or_else(|| format!("pixels::index: tile ({x},{y}) is out of bounds"))?
                .push(id);
        }
    }
    Ok(())
}

fn object_feature_ranges(
    structural: &super::verify::StructuralProgram,
) -> Result<Vec<ObjectFeatureRange>, String> {
    let mut ranges = Vec::new();
    for object in &structural.objects.objects {
        let ids = structural
            .features
            .iter()
            .filter(|feature| feature.object == object.id)
            .map(|feature| feature.id)
            .collect::<Vec<_>>();
        let Some(first) = ids.first().copied() else {
            return Err(format!(
                "pixels::index: object {} has no feature range",
                object.id
            ));
        };
        if ids
            .iter()
            .enumerate()
            .any(|(offset, id)| id.0 != first.0 + offset as u32)
        {
            return Err(format!(
                "pixels::index: object {} features are not contiguous",
                object.id
            ));
        }
        ranges.push(ObjectFeatureRange {
            object: object.id,
            first,
            count: u32::try_from(ids.len())
                .map_err(|_| "P015: object feature range count overflow".to_string())?,
        });
    }
    Ok(ranges)
}

fn checked_index_bytes(indexes: &[&CompressedIndex]) -> Result<u64, String> {
    indexes.iter().try_fold(0_u64, |bytes, index| {
        bytes
            .checked_add(index.encoded_bytes()?)
            .ok_or_else(|| "P015: total index byte count overflow".to_string())
    })
}

pub fn compile(
    graph: &super::symbolic::SymbolicGraph,
    config: &super::config::RendererConfig,
    structural: &super::verify::StructuralProgram,
    derivatives: &super::derivatives::DerivativePrograms,
    spans: &[ProjectedFeatureSpan],
    events: &super::events::EventPrograms,
    competitions: &super::competition::CompetitionPrograms,
) -> Result<LocalIndexes, String> {
    let tiles_x = config.width.div_ceil(TILE_WIDTH_V1);
    let tiles_y = config.height.div_ceil(TILE_HEIGHT_V1);
    let tile_count = tiles_x
        .checked_mul(tiles_y)
        .ok_or_else(|| "P015: renderer tile count overflow".to_string())?;
    let tile_count = usize::try_from(tile_count)
        .map_err(|_| "P015: renderer tile count exceeds usize".to_string())?;
    let empty_cells = || vec![Vec::<u32>::new(); tile_count];
    let mut tile_features = empty_cells();
    for span in spans {
        add_tile_span(&mut tile_features, span.tiles, tiles_x, span.feature.0)?;
    }
    let mut tile_events = empty_cells();
    for event in &events.generators {
        add_tile_span(&mut tile_events, event.tiles, tiles_x, event.id.0)?;
    }
    let mut tile_competitions = empty_cells();
    for pair in &competitions.pairs {
        add_tile_span(&mut tile_competitions, pair.tiles, tiles_x, pair.id.0)?;
    }
    let row_block_count =
        usize::try_from(tiles_y).map_err(|_| "P015: row-block count exceeds usize".to_string())?;
    let mut row_block_repeats = vec![Vec::<u32>::new(); row_block_count];
    let mut repeat_id = 0_u32;
    for template in &structural.repeats {
        for instance in &template.instances {
            let object_spans = structural
                .features
                .iter()
                .filter(|feature| feature.object == instance.object)
                .map(|feature| &spans[feature.id.index()])
                .collect::<Vec<_>>();
            let row_start = object_spans
                .iter()
                .map(|span| span.tiles.y.start)
                .min()
                .unwrap_or(0);
            let row_end = object_spans
                .iter()
                .map(|span| span.tiles.y.end)
                .max()
                .unwrap_or(0);
            for row in row_start..row_end {
                let row = usize::try_from(row)
                    .map_err(|_| "P015: repeat row index exceeds usize".to_string())?;
                row_block_repeats[row].push(repeat_id);
            }
            repeat_id = repeat_id
                .checked_add(1)
                .ok_or_else(|| "P015: repeat index ID overflow".to_string())?;
        }
    }
    let object_features = object_feature_ranges(structural)?;
    let feature_derivatives = derivatives
        .bundles
        .iter()
        .enumerate()
        .map(|(index, bundle)| {
            if bundle.feature.index() != index {
                return Err(format!(
                    "pixels::index: derivative bundle {} is not in feature order",
                    bundle.id
                ));
            }
            Ok(bundle.id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let material_programs = structural
        .objects
        .identities
        .iter()
        .map(|identity| MaterialProgramIndex {
            identity_set: identity.id,
            material: graph.material_root,
        })
        .collect::<Vec<_>>();
    let mut tile_lights = empty_cells();
    for cell in &mut tile_lights {
        cell.extend(0..config.light_capacity);
    }
    let mut tile_probes = empty_cells();
    if config.probes_enabled {
        for cell in &mut tile_probes {
            cell.push(0);
        }
    }
    let tile_features = compress(tile_features, "feature")?;
    let tile_events = compress(tile_events, "event")?;
    let tile_competitions = compress(tile_competitions, "competition")?;
    let row_block_repeats = compress(row_block_repeats, "repeat")?;
    let tile_lights = compress(tile_lights, "light")?;
    let tile_probes = compress(tile_probes, "probe")?;
    let table_bytes = checked_index_bytes(&[
        &tile_features,
        &tile_events,
        &tile_competitions,
        &row_block_repeats,
        &tile_lights,
        &tile_probes,
    ])?;
    let direct_bytes = u64::try_from(object_features.len())
        .ok()
        .and_then(|count| count.checked_mul(12))
        .and_then(|bytes| {
            u64::try_from(feature_derivatives.len())
                .ok()
                .and_then(|count| count.checked_mul(4))
                .and_then(|derivatives| bytes.checked_add(derivatives))
        })
        .and_then(|bytes| {
            u64::try_from(material_programs.len())
                .ok()
                .and_then(|count| count.checked_mul(8))
                .and_then(|materials| bytes.checked_add(materials))
        })
        .ok_or_else(|| "P015: direct index byte count overflow".to_string())?;
    let bytes = table_bytes
        .checked_add(direct_bytes)
        .ok_or_else(|| "P015: total local index byte count overflow".to_string())?;
    Ok(LocalIndexes {
        tiles_x,
        tiles_y,
        tile_features,
        tile_events,
        tile_competitions,
        row_block_repeats,
        object_features,
        feature_derivatives,
        material_programs,
        tile_lights,
        tile_probes,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_offset_count_plus_contiguous_slice() {
        let index = compress(vec![vec![3, 1], vec![], vec![2]], "test").unwrap();
        assert_eq!(index.lookup(0).unwrap(), &[1, 3]);
        assert_eq!(index.lookup(1).unwrap(), &[]);
        assert_eq!(index.lookup(2).unwrap(), &[2]);
        assert!(index.lookup(3).unwrap_err().contains("outside"));
    }

    #[test]
    fn duplicate_ids_inside_one_cell_fail_closed() {
        assert!(
            compress(vec![vec![1, 1]], "test")
                .unwrap_err()
                .contains("duplicate")
        );
    }
}
