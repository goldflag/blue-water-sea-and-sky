# Third-party code in naval-sky

The atmosphere (`shaders/bruneton.wgsl`, `shaders/precompute.wgsl`, the table passes in `src/tables.rs`) is a
WGSL port of takram's atmosphere package (three-geospatial, `packages/atmosphere`: `src/shaders/bruneton/*.glsl`,
`src/shaders/precompute/*.frag`, `src/PrecomputedTexturesGenerator.ts`, `src/AtmosphereParameters.ts`, at commit
b012ad06), which is itself based on Eric Bruneton's Precomputed Atmospheric Scattering. Changes made in the port
are listed in `native/docs/sky.md`. The atmosphere ships no data files: its tables are computed on the GPU.

The stars (`data/bright_stars.tsv`) are the Yale Bright Star Catalogue, 5th revised edition (Hoffleit D., Warren
W.H. Jr., 1991), as distributed by the Centre de Données astronomiques de Strasbourg, catalogue V/50
(https://cdsarc.cds.unistra.fr/ftp/cats/V/50/). `data/build.ts` writes the table from it and says what it changes.

## takram three-geospatial (MIT)

https://github.com/takram-design-engineering/three-geospatial

The MIT License (MIT)

Copyright (c) 2024 Shota Matsuda

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of
the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NON-INFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Precomputed Atmospheric Scattering (BSD 3-Clause)

https://github.com/ebruneton/precomputed_atmospheric_scattering

Copyright (c) 2017 Eric Bruneton
All rights reserved.

Precomputed Atmospheric Scattering
Copyright (c) 2008 INRIA
All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
   disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
   following disclaimer in the documentation and/or other materials provided with the distribution.
3. Neither the name of the copyright holders nor the names of its contributors may be used to endorse or promote
   products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
