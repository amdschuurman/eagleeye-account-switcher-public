// The Battle.net and Overwatch emblems are Blizzard trademarks, so this repository does not carry them.
// `npm run icons` downloads them into temp/brand/ (gitignored). Vite looks there at build time: a build made after
// `npm run icons` bundles them, a build without them gets `undefined` here and the UI simply shows no emblem.
const found = import.meta.glob<string>("/temp/brand/*.svg", { eager: true, query: "?url", import: "default" });

export const battlenetEmblem: string | undefined = found["/temp/brand/battlenet.svg"];
export const overwatchEmblem: string | undefined = found["/temp/brand/overwatch.svg"];
