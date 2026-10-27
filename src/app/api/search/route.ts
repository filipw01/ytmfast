import { NextResponse } from "next/server";
import { getDemoSearch } from "@/lib/demo-data";
import { fetchSearch } from "@/lib/ytmusic";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url);
  const query = (searchParams.get("q") || "").trim();
  const forceDemo = searchParams.get("demo") === "1";

  if (!query) {
    return NextResponse.json({
      source: forceDemo ? "demo" : "live",
      query: "",
      songs: [],
      albums: [],
      playlists: [],
      artists: [],
    });
  }

  if (forceDemo) {
    return NextResponse.json(getDemoSearch(query));
  }

  try {
    return NextResponse.json(await fetchSearch(query));
  } catch (error) {
    console.error("search failed", error);
    return NextResponse.json({
      ...getDemoSearch(query),
      error:
        error instanceof Error
          ? error.message
          : "Search unavailable; showing demo results.",
    });
  }
}
