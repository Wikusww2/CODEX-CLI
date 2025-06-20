// This file is a shim to provide a type declaration for a module that is
// missing one. This is a workaround to satisfy the TypeScript compiler.
declare module 'openai/resources/responses/responses.mjs' {
  // We are defining ResponseItem as 'any' here to avoid breaking the build.
  // This is not ideal, but it's a pragmatic solution for now.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  export type ResponseItem = any;
}
