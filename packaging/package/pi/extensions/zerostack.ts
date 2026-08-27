import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.registerCommand("zerostack", {
    description: "Report ZeroStack pre-release scaffold status",
    handler: async (_args: string, ctx) => {
      const message = [
        "ZeroStack Pi extension is a pre-release scaffold.",
        "Version 0.0.0 does not register native tools and does not claim a published release.",
        "",
        "What works today:",
        "- Node binding lives in bindings/node as @zerostack/zero-kernel (loader.js and zero-kernel.d.ts)",
        "- Rust crates live under crates/zerostack, including zero-kernel and zero-kernel-node",
        "- Homebrew head formula is packaging/package/homebrew/zerostack.rb",
        "",
        "Next steps:",
        "- Build the Rust workspace with cargo build",
        "- Build the Node prebuild with packaging/package/npm/build-prebuild.sh",
        "- Use demo/run.js for a real ZeroKernel cell that combines z.read and z.find",
      ].join("\n");

      ctx.ui.notify(message, "info");
    },
  });
}
