/* dov-module
format = 1
id = "dov.fold"
name = "Folder"
category = "process"
description = "Wavefolder: gain past full scale reflects back, turning ramps into bands."
kernel = "pointwise"

[[input]]
id = "sig"
name = "In"
kind = "mono"

[[output]]
id = "out"
name = "Out"
kind = "mono"

[[param]]
id = "gain"
name = "Gain"
range = [1.0, 8.0]
default = 2.0
cv = "video"

[[param]]
id = "bias"
name = "Bias"
range = [-1.0, 1.0]
default = 0.0
cv = "video"
*/

fn process(ctx: Ctx, inp: Inputs, p: Params) -> Outputs {
    var o: Outputs;
    o.out = fold(inp.sig * p.gain + p.bias);
    return o;
}
