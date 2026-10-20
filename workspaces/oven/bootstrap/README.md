# dev.8 bootstrap helpers

These helpers operate on the lane named by `DEV8_LANE`; they refuse an unset lane. The lane already contains the frozen stage-zero bootstrap and prepared registry closure described in DD-0008.

`install-toolchain.sh` writes `incan.toolchain/1`, including `registry_units`, the explicit bootstrap stand-in for incan.pub's baked registry Loaves. Copy these helpers into the lane's own `bootstrap/scripts` directory before using them. Do not overwrite the shared bootstrap scripts.

`relink-driver.sh` and `refresh-runtime.sh` use the installed toolchain's registry closure through ordinary `incan build`. `compile-one.sh` asks Oven to describe and prepare a unit, filters uncompiled standard packages during bootstrap, and passes that description to the installed compiler. It never discovers or chooses Rust extern artifacts itself.
