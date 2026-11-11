# SSTable

**Status:**

## Purpose

<!-- What is an SSTable, and why is it immutable? -->

## File layout

<!-- Data blocks, bloom filter, index, footer: in what order? Draw it. -->

## Data blocks

<!-- Block size and why. How are entries laid out inside a block? What if a value is bigger than a block? Block CRC? -->

## Index

<!-- What is stored per block? First key or last key, and why? -->

## Footer

<!-- Every field with its width and byte order. Total footer size. -->

## Sizing

<!-- Target SSTable size and the math behind it. -->

## Reading a key

<!-- Step by step: how a get uses the footer, bloom filter, index, and block. -->

## Interface

<!-- Writer and reader functions (names and signatures, no bodies). -->

## Tests

## Open questions

## See also

DESIGN.md, section 7 · [BLOOM_FILTER.md](BLOOM_FILTER.md)
