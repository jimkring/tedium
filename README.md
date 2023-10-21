# TDMS Library

This is a library to support the TDMS file format as defined by NI in https://www.ni.com/en/support/documentation/supplemental/07/tdms-file-format-internal-structure.html

The design and approach to this library is to prioritise:

1. Performance - I've long felt some common cases are not handled optimally in NI's reference implementation. Initial testing shows as much as a 10x improvement in standard reads.
2. Usability - This library utilises the type system of Rust to make it easy to do the right thing.
3. Full Specification - See the Supported Files section below. I'm aiming for 100% coverage of files that I can see are possible to create with other APIs, not 100% coverage of what the spec says as there are features that I don't believe are used or even possible.


## Supported Files

Once the various types are supported we expect to be able to support all TDMS files.

There is a point of confusion however that the file format allows the expression of files that as far as I can find, cannot be created by clients.

This greatly simplify the software so we do make the following assumptions:

* All channels in the same data segment have the same length. (Note: This is not the same as all channels in a group having the same length)

## Library Structure

If you look through the library you will see some key modules:

* **io:** This wraps the direct I/O traits with wrappers to handle TDMS specific formatting. Notably the data types supported and the endianess.
* **raw_data:** This module wraps the logic for reading channel data from the raw segments. A key goal for this library was to maximize performance so this includes a stage to plan an optimal read structure (in `records.rs`) and then execute that against the two forms so we minimize disk reads.
* **index:** This is the in memory index structure that is built when we first scan a file and can use to look up properties and segments.
* **meta_data:** This handles reading the segment headers out of the file which can be ingested into the index.