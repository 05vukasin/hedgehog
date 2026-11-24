# calamine 0.37.0, patched

Upstream: https://github.com/tafia/calamine (MIT, `LICENSE-MIT.md` retained).
Hedgehog patches this crate in through the workspace `[patch.crates-io]`; it is not a workspace
member. Every change from 0.37.0 is marked with a `// hedgehog:` comment at the spot, and every
fix for a hostile file has a regression test in `core/tests/hostile_files.rs` that builds the
input from bytes (no fuzzer outputs are checked in). This file lists the patches by file.

## Robustness (hostile and broken files)

- `src/utils.rs`: the fixed-size readers (`read_u16`, `read_u32`, `read_f64`, ...) read the bytes
  that are there and treat missing ones as zero; `to_u32` ignores a partial last value. Many
  record parsers read fields without checking the record's length, and a short record panicked.
- `src/cfb.rs` (OLE containers, xls): a sector chain visits each sector at most once (a loop grew
  memory until the process aborted), a sector id past the FAT is an error, a file cut short
  keeps only whole entries and sectors.
- `src/xls.rs`: short and empty records (BOF, EXTERNSHEET, MERGECELLS, Lbl, CONTINUE, ...), sheet
  offsets past the stream, untrusted declared sizes and counts, `col_last < col_first`; every
  formula token is bounds-checked (`need`), and row + 1 no longer overflows u16.
- `src/xlsx/mod.rs`: a `sharedStrings.xml` `uniqueCount` is not trusted for the allocation (it
  reserved up to 51 GB); inverted ranges and overlong cell references are errors, not
  wraparounds.
- `src/lib.rs`: `Dimensions::len` of an inverted dimension is 0 (it overflowed).
- `src/xlsb/mod.rs` and `src/xlsb/cells_reader.rs`: see [xlsb](#xlsb) below.

## Features and fixes for real files

- `src/xls.rs`: cells kept sparse, dense ranges built on request; the cells and formulas of a
  sheet as read; BIFF8 strings are UTF-16 whatever the CODEPAGE says (`src/cfb.rs`, the string
  decoder).
- `src/datatype.rs`: 1904-system serials are converted to the 1900 system.
- `src/formats.rs`: East Asian weekday formats (`aaa`, `aaaa`) are dates.
- `src/xlsx/mod.rs`, `src/xlsx/cells_reader.rs`, `src/xlsx/parallel.rs` (new): large sheets and
  shared string tables read on several threads with the same result.
- `src/ods.rs`: large tables on several threads; zip entries with backslash separators.
- `src/xlsb/mod.rs`: the workbook relationships part is found through the zip path cache
  (backslash entry names, B52).

## xlsb

Found by the fuzzer's `xlsb` target; each site below panicked, and is now a calamine error
(`XlsbError::Len`, new: "Invalid <record> length, expected at least N, found M", or an existing
variant) unless said otherwise.

`src/xlsb/mod.rs`

- `XlsbError::Len` and the helpers `slice`, `tail` and `byte`: `buf[a..b]`, `buf[a..]` and
  `buf[i]`, or `Err(Len)` where indexing panicked.
- `RecordIter::fill_buffer`: a record's declared length (up to 256 MB, from 4 bytes) zero-filled
  the buffer before the read failed; the buffer now grows by the bytes the part really holds.
  The result is the same as before: the record in `buf[..len]`, older bytes after it.
- Every record is read from the record itself (`&buf[..len]`). The buffer keeps the bytes of
  longer earlier records, and a short record read them: an empty BrtSSTItem copied the previous
  string again (two bytes of input for a copy of a string of any length). A valid file reads
  the same, since its fields lie inside their records.
- `read_styles`: BrtFmt shorter than 2 bytes, BrtXF shorter than 4.
- `read_shared_strings`: BrtBeginSst shorter than 8 bytes; BrtSSTItem bounds-checked.
- `read_workbook`: an empty BrtWbProp; BrtBundleSh shorter than 8 bytes, a relId length past
  the record, a relId missing from `workbook.bin.rels` (`relationships[&relid]`, now
  `Unrecognized`), a name offset past the record; BrtExternSheet shorter than 4 bytes or with a
  short last XTI, and its untrusted count no longer reserves memory (up to 24 MB for a vector
  that was replaced right after); BrtName shorter than 9 bytes or with a formula longer than
  the record.
- `wide_str`: the length computation cannot overflow on 32-bit targets.
- `parse_formula`: every token checks its length first (`need`), as in the xls reader: PtgRef3d,
  PtgArea3d, PtgRefErr3d, PtgAreaErr3d, PtgExp, PtgStr, PtgExtended (PtgList, PtgSxName),
  PtgAttr, PtgErr, PtgBool, PtgInt, PtgNum, PtgArray, PtgFunc, PtgFuncVar, PtgName, PtgRef,
  PtgArea, PtgRefErr, PtgAreaErr, PtgMemFunc, PtgNameX.
  - An XTI index past the BrtExternSheet entries (`sheets[ixti]`) is the sheet `#REF`, as in
    the xls reader (no error: the rest of the formula reads).
  - PtgFuncVar's function index was not checked against FTAB; PtgFunc's check was `>` and let
    the index FTAB_LEN through (`FTAB_ARGC[485]`). Both are `>=` now (`IfTab`).
  - PtgName index 0 (one-based) and row u32::MAX + 1 overflowed with overflow checks on; they
    wrap as a release build did (no name; row 0), so the output is unchanged.
  - Nested PtgMemFunc, each a recursive call, is limited to 32 levels (an rgce nested ~21,000
    deep could overflow the stack); formula text and operand stack to 64 KiB (Excel's limit is
    8,192 characters; a few bytes per reference to a long sheet name gave gigabytes, and
    parentheses, each an insert at the front, took quadratic time). Both are `Unrecognized`.
  - A function's arguments are taken with a checked slice.
- `worksheet_formula`: reserves at most 100,000 cells for the untrusted dimension (was 1,000,000,
  32 MB), as `worksheet_range_ref` does.
- `cell_format`: a record shorter than 7 bytes has no style.

`src/xlsb/cells_reader.rs`

- `XlsbCellsReader::new`: BrtWsDim shorter than 16 bytes (`buf[..16]`).
- `next_cell`: fields read from the record and bounds-checked (BrtCellRk, BrtCellError,
  BrtCellBool, BrtCellReal, BrtCellSt, BrtCellIsst); a shared string index past the table
  (`strings[isst]`) is an error.
- `next_formula`: BrtFmlaString, BrtFmlaNum, BrtFmlaBool and BrtFmlaError whose string length or
  token count reaches past the record (`buf[14 + 2 * cch..]`, `formula[4..4 + cce]`), or that are
  shorter than their fixed fields.
- `column`: a cell column past 0x3FFF (Excel's 16,384 columns, MS-XLSB 2.5.9) is an error. A
  column up to u32::MAX was passed on and overflowed hedgehog's A1 addresses and layout.

Not changed (the output of valid files stays the same; candidates for later): a chart sheet in a
workbook makes the whole xlsb unreadable (its part has no BrtWsDim); records of unknown type in
the defined-name section of `workbook.bin` are not skipped by their length, so their bytes are
read as records (upstream's test files `unknown_record_before_names.xlsb` and
`truncated_name_record.xlsb` panicked here before; they are errors now).
