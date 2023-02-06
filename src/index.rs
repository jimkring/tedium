//! The registry module creates the data structure which acts as
//! an in memory index of the file contents.
//!
//! This will store known objects and their properties and data locations.

use std::collections::HashMap;

use crate::error::TdmsError;
use crate::file_types::{
    ObjectMetaData, PropertyValue, RawDataIndex, RawDataMeta, SegmentMetaData,
};
use crate::raw_data::DataBlock;

/// A store for a given channel point to the data block with its data and the index within that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLocation {
    /// The index of the data block with the data in.
    pub data_block: usize,
    /// The channel index in that block.
    pub channel_index: usize,
}

///Represents actual data formats that can store data.
#[derive(Clone, PartialEq, Eq, Debug)]
enum DataFormat {
    RawData(RawDataMeta),
}

impl DataFormat {
    fn from_index(index: &RawDataIndex) -> Option<Self> {
        match index {
            RawDataIndex::RawData(raw_meta) => Some(DataFormat::RawData(raw_meta.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
struct ObjectData {
    path: String,
    properties: HashMap<String, PropertyValue>,
    data_locations: Vec<DataLocation>,
    latest_data_format: Option<DataFormat>,
}

impl ObjectData {
    //todo: this can be more efficient
    fn from_metadata(meta: &ObjectMetaData) -> Self {
        let mut new = Self {
            path: meta.path.clone(),
            properties: HashMap::new(),
            data_locations: vec![],
            latest_data_format: None,
        };

        new.update(meta);

        new
    }
    fn update(&mut self, other: &ObjectMetaData) {
        for (name, value) in other.properties.iter() {
            self.properties.insert(name.clone(), value.clone());
        }
        if let Some(format) = DataFormat::from_index(&other.raw_data_index) {
            self.latest_data_format = Some(format)
        }
    }

    fn add_data_location(&mut self, location: DataLocation) {
        self.data_locations.push(location);
    }

    fn get_all_properties(&self) -> Vec<(&String, &PropertyValue)> {
        self.properties.iter().collect()
    }
}

#[derive(Debug, Clone)]
struct ActiveObject {
    path: String,
}

impl ActiveObject {
    fn update(&mut self, _meta: &ObjectMetaData) {}

    fn get_object_data<'b, 'c>(&'b self, registry: &'c ObjectRegistry) -> &'c ObjectData {
        registry
            .get(&self.path)
            .expect("Should always have a registered version of active object")
    }
    fn get_object_data_mut<'b, 'c>(
        &'b self,
        registry: &'c mut ObjectRegistry,
    ) -> &'c mut ObjectData {
        registry
            .get_mut(&self.path)
            .expect("Should always have a registered version of active object")
    }
}

type ObjectRegistry = HashMap<String, ObjectData>;

#[derive(Default, Debug, Clone)]
pub struct FileScanner {
    active_objects: Vec<ActiveObject>,
    object_registry: ObjectRegistry,
    data_blocks: Vec<DataBlock>,
    next_segment_start: u64,
}

impl FileScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_segment_to_index(&mut self, segment: SegmentMetaData) {
        //Basic procedure.
        //1. If new object list is set, clear active objects.
        //2. Update the active object list - adding new objects or updating properties and data locations for existing objects.

        if segment.toc.contains_new_object_list {
            self.deactivate_all_objects();
        }

        segment
            .objects
            .iter()
            .for_each(|obj| match obj.raw_data_index {
                RawDataIndex::None => self.update_meta_object(obj),
                _ => self.update_or_activate_data_object(obj),
            });

        if segment.toc.contains_raw_data {
            let data_block = DataBlock::from_segment(
                &segment,
                self.next_segment_start,
                self.get_active_raw_data_meta(),
            );

            self.insert_data_block(data_block);
        }

        self.next_segment_start += segment.total_size_bytes();
    }

    fn get_active_raw_data_meta(&self) -> Vec<RawDataMeta> {
        self.active_objects
            .iter()
            .map(|ao| {
                ao.get_object_data(&self.object_registry)
                    .latest_data_format
                    .clone()
                    .expect("Getting data format from object that never had one")
            })
            .map(|format| match format {
                DataFormat::RawData(raw) => raw,
            })
            .collect()
    }

    fn insert_data_block(&mut self, block: DataBlock) {
        let data_index = self.data_blocks.len();
        self.data_blocks.push(block);

        for (channel_index, active_object) in self.active_objects.iter_mut().enumerate() {
            let location = DataLocation {
                data_block: data_index,
                channel_index,
            };
            active_object
                .get_object_data_mut(&mut self.object_registry)
                .add_data_location(location);
        }
    }

    /// Consumes the object and makes it inactive.
    ///
    /// Panics if the object was already listed as inactive.
    fn deactivate_all_objects(&mut self) {
        self.active_objects.clear();
    }

    /// Activate Data Object
    ///
    /// Adds the object by path to the active objects. Creates it if it doesn't exist.
    fn update_or_activate_data_object(&mut self, object: &ObjectMetaData) {
        let matching_active = self
            .active_objects
            .iter_mut()
            .find(|active_object| active_object.path == object.path);

        match matching_active {
            Some(active_object) => {
                active_object.update(object);
                active_object
                    .get_object_data_mut(&mut self.object_registry)
                    .update(object);
            }
            None => {
                self.active_objects.push(ActiveObject {
                    path: object.path.clone(),
                });
                self.update_meta_object(object);
            }
        }
    }

    /// Update Meta Only Object
    ///
    /// Update an object which contains no data.
    fn update_meta_object(&mut self, object: &ObjectMetaData) {
        match self.object_registry.get_mut(&object.path) {
            Some(found_object) => found_object.update(object),
            None => {
                let object_data = ObjectData::from_metadata(object);
                let old = self
                    .object_registry
                    .insert(object_data.path.clone(), object_data);
                assert!(
                    matches!(old, None),
                    "Should not be possible to be replacing an existing object."
                );
            }
        }
    }

    pub fn into_index(mut self) -> Index {
        self.deactivate_all_objects();

        Index {
            objects: self.object_registry,
            data_blocks: self.data_blocks,
        }
    }
}

pub struct Index {
    objects: ObjectRegistry,
    data_blocks: Vec<DataBlock>,
}

impl Index {
    pub fn get_object_properties(&self, path: &str) -> Option<Vec<(&String, &PropertyValue)>> {
        self.objects
            .get(path)
            .map(|object| object.get_all_properties())
    }

    pub fn get_object_property(
        &self,
        path: &str,
        property: &str,
    ) -> Result<Option<&PropertyValue>, TdmsError> {
        let property = self
            .objects
            .get(path)
            .ok_or_else(|| TdmsError::MissingObject(path.to_string()))?
            .properties
            .get(property);

        Ok(property)
    }

    pub fn get_channel_data_positions(&self, path: &str) -> Option<&[DataLocation]> {
        self.objects
            .get(path)
            .map(|object| &object.data_locations[..])
    }

    pub fn get_data_block(&self, index: usize) -> Option<&DataBlock> {
        self.data_blocks.get(index)
    }
}

#[cfg(test)]
mod tests {
    use crate::file_types::DataTypeRaw;
    use crate::file_types::ObjectMetaData;
    use crate::file_types::PropertyValue;
    use crate::file_types::RawDataIndex;
    use crate::file_types::RawDataMeta;
    use crate::file_types::ToC;
    use crate::raw_data::{DataLayout, Endianess};

    use super::*;

    #[test]
    fn test_single_segment() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);

        let registry = scanner.into_index();

        let group_properties = registry.get_object_properties("group").unwrap();
        assert_eq!(
            group_properties,
            &[(&"Prop".to_string(), &PropertyValue::I32(-51))]
        );
        let ch1_properties = registry.get_object_properties("group/ch1").unwrap();
        assert_eq!(
            ch1_properties,
            &[(&String::from("Prop1"), &PropertyValue::I32(-1))]
        );
        let ch2_properties = registry.get_object_properties("group/ch2").unwrap();
        assert_eq!(
            ch2_properties,
            &[(&"Prop2".to_string(), &PropertyValue::I32(-2))]
        );

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[DataLocation {
                data_block: 0,
                channel_index: 0
            }]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[DataLocation {
                data_block: 0,
                channel_index: 1
            }]
        );
    }

    #[test]
    fn correctly_generates_the_data_block() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);

        let registry = scanner.into_index();

        let expected_data_block = DataBlock {
            start: 48,
            length: 480,
            layout: DataLayout::Contigious,
            channels: vec![
                RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                },
                RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                },
            ],
            byte_order: Endianess::Little,
        };

        let block = registry.get_data_block(0).unwrap();
        assert_eq!(block, &expected_data_block);
    }

    #[test]
    fn correctly_generates_the_data_block_same_as_previous() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };

        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xA),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![],
                    raw_data_index: RawDataIndex::MatchPrevious,
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![],
                    raw_data_index: RawDataIndex::MatchPrevious,
                },
            ],
        };
        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let expected_data_block = DataBlock {
            start: 576,
            length: 480,
            layout: DataLayout::Contigious,
            channels: vec![
                RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                },
                RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                },
            ],
            byte_order: Endianess::Little,
        };

        let block = registry.get_data_block(1).unwrap();
        assert_eq!(block, &expected_data_block);
    }

    #[test]
    fn correctly_generates_the_data_block_same_as_previous_new_list() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };

        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![],
                    raw_data_index: RawDataIndex::MatchPrevious,
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![],
                    raw_data_index: RawDataIndex::MatchPrevious,
                },
            ],
        };
        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let expected_data_block = DataBlock {
            start: 576,
            length: 480,
            layout: DataLayout::Contigious,
            channels: vec![
                RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                },
                RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                },
            ],
            byte_order: Endianess::Little,
        };

        let block = registry.get_data_block(1).unwrap();
        assert_eq!(block, &expected_data_block);
    }

    #[test]
    fn does_not_generate_block_for_meta_only() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0x2),
            next_segment_offset: 20,
            raw_data_offset: 20,
            objects: vec![ObjectMetaData {
                path: "group".to_string(),
                properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                raw_data_index: RawDataIndex::None,
            }],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);

        let registry = scanner.into_index();

        let block = registry.get_data_block(0);
        assert_eq!(block, None);
    }

    #[test]
    fn updates_existing_properties() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            // 2 is meta data only.
            toc: ToC::from_u32(0x2),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-52))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::None,
                },
            ],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);
        let index = scanner.into_index();

        let group_properties = index.get_object_properties("group").unwrap();
        assert_eq!(
            group_properties,
            &[(&"Prop".to_string(), &PropertyValue::I32(-52))]
        );
        let ch1_properties = index.get_object_properties("group/ch1").unwrap();
        assert_eq!(
            ch1_properties,
            &[(&"Prop1".to_string(), &PropertyValue::I32(-2))]
        );
    }

    /// This tests the second optimisation on the NI article.
    #[test]
    fn can_update_properties_with_no_changes_to_data_layout() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xA),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![ObjectMetaData {
                path: "group/ch1".to_string(),
                properties: vec![("Prop1".to_string(), PropertyValue::I32(-2))],
                raw_data_index: RawDataIndex::MatchPrevious,
            }],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let group_properties = registry.get_object_properties("group").unwrap();
        assert_eq!(
            group_properties,
            &[(&"Prop".to_string(), &PropertyValue::I32(-51))]
        );
        let ch1_properties = registry.get_object_properties("group/ch1").unwrap();
        assert_eq!(
            ch1_properties,
            &[(&String::from("Prop1"), &PropertyValue::I32(-2))]
        );
        let ch2_properties = registry.get_object_properties("group/ch2").unwrap();
        assert_eq!(
            ch2_properties,
            &[(&"Prop2".to_string(), &PropertyValue::I32(-2))]
        );

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 0
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 0
                }
            ]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 1
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 1
                }
            ]
        );
    }

    /// This tests that the previous active list is maintained with no objects updated.
    #[test]
    fn can_keep_data_with_no_objects_listed() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xA),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 0
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 0
                }
            ]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 1
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 1
                }
            ]
        );
    }

    /// This tests that the previous active list is maintained with no metadata updated.
    #[test]
    fn can_keep_data_with_no_metadata_in_toc() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0x8),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 0
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 0
                }
            ]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 1
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 1
                }
            ]
        );
    }

    #[test]
    fn can_add_channel_to_active_list() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xA),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![ObjectMetaData {
                path: "group/ch3".to_string(),
                properties: vec![("Prop3".to_string(), PropertyValue::I32(-3))],
                raw_data_index: RawDataIndex::RawData(RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                }),
            }],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let ch3_properties = registry.get_object_properties("group/ch3").unwrap();
        assert_eq!(
            ch3_properties,
            &[(&"Prop3".to_string(), &PropertyValue::I32(-3))]
        );

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 0
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 0
                }
            ]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 1
                },
                DataLocation {
                    data_block: 1,
                    channel_index: 1
                }
            ]
        );
        let ch3_data = registry.get_channel_data_positions("group/ch3").unwrap();
        assert_eq!(
            ch3_data,
            &[DataLocation {
                data_block: 1,
                channel_index: 2
            }]
        );
    }

    #[test]
    fn can_replace_the_existing_list() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![ObjectMetaData {
                path: "group/ch3".to_string(),
                properties: vec![("Prop3".to_string(), PropertyValue::I32(-3))],
                raw_data_index: RawDataIndex::RawData(RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                }),
            }],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);

        let registry = scanner.into_index();

        let ch3_properties = registry.get_object_properties("group/ch3").unwrap();
        assert_eq!(
            ch3_properties,
            &[(&"Prop3".to_string(), &PropertyValue::I32(-3))]
        );

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[DataLocation {
                data_block: 0,
                channel_index: 0
            },]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[DataLocation {
                data_block: 0,
                channel_index: 1
            },]
        );
        let ch3_data = registry.get_channel_data_positions("group/ch3").unwrap();
        assert_eq!(
            ch3_data,
            &[DataLocation {
                data_block: 1,
                channel_index: 0
            }]
        );
    }

    #[test]
    fn can_re_add_channel_to_active_list() {
        let segment = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![
                ObjectMetaData {
                    path: "group".to_string(),
                    properties: vec![("Prop".to_string(), PropertyValue::I32(-51))],
                    raw_data_index: RawDataIndex::None,
                },
                ObjectMetaData {
                    path: "group/ch1".to_string(),
                    properties: vec![("Prop1".to_string(), PropertyValue::I32(-1))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
                ObjectMetaData {
                    path: "group/ch2".to_string(),
                    properties: vec![("Prop2".to_string(), PropertyValue::I32(-2))],
                    raw_data_index: RawDataIndex::RawData(RawDataMeta {
                        data_type: DataTypeRaw::DoubleFloat,
                        number_of_values: 1000,
                        total_size_bytes: None,
                    }),
                },
            ],
        };
        let segment2 = SegmentMetaData {
            toc: ToC::from_u32(0xE),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![ObjectMetaData {
                path: "group/ch3".to_string(),
                properties: vec![("Prop3".to_string(), PropertyValue::I32(-3))],
                raw_data_index: RawDataIndex::RawData(RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                }),
            }],
        };
        let segment3 = SegmentMetaData {
            toc: ToC::from_u32(0xA),
            next_segment_offset: 500,
            raw_data_offset: 20,
            objects: vec![ObjectMetaData {
                path: "group/ch1".to_string(),
                properties: vec![("Prop3".to_string(), PropertyValue::I32(-3))],
                raw_data_index: RawDataIndex::RawData(RawDataMeta {
                    data_type: DataTypeRaw::DoubleFloat,
                    number_of_values: 1000,
                    total_size_bytes: None,
                }),
            }],
        };

        let mut scanner = FileScanner::new();
        scanner.add_segment_to_index(segment);
        scanner.add_segment_to_index(segment2);
        scanner.add_segment_to_index(segment3);

        let registry = scanner.into_index();

        let ch1_data = registry.get_channel_data_positions("group/ch1").unwrap();
        assert_eq!(
            ch1_data,
            &[
                DataLocation {
                    data_block: 0,
                    channel_index: 0
                },
                DataLocation {
                    data_block: 2,
                    channel_index: 1
                }
            ]
        );
        let ch2_data = registry.get_channel_data_positions("group/ch2").unwrap();
        assert_eq!(
            ch2_data,
            &[DataLocation {
                data_block: 0,
                channel_index: 1
            },]
        );
        let ch3_data = registry.get_channel_data_positions("group/ch3").unwrap();
        assert_eq!(
            ch3_data,
            &[
                DataLocation {
                    data_block: 1,
                    channel_index: 0
                },
                DataLocation {
                    data_block: 2,
                    channel_index: 0
                }
            ]
        );
    }
}
