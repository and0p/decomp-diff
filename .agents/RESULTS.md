# Results

Measurements on the [Thwaite example](../examples/thwaite/), the end-to-end test.

- All 12 segments reproduce their recorded hashes on Linux x86_64 (where they were recorded) and on macOS arm64 (Apple clang 17), both from cached parent states and from scratch. The ROM builds byte-identical on both.
- Coverage from 11,140 frames: 114 of 134 functions entered, 835 of 1,087 basic blocks, 3,733 of 4,656 instructions (80.2%).
- The 5-segment smallest set keeps all of it.
- Playback from cached parent states runs at about 480 frames per second on one Linux x86_64 cloud core and 717 on an Apple silicon MacBook Air (8 to 12 times real time). The coverage patch costs about 5% against stock Mesen, with identical hashes.
- The gaps are what you would expect from short, unskilled play: balloons and MIRVs, end-of-round scoring, and rebuilding houses and silos. Each needs a segment that survives a full round.
