import { NextResponse } from "next/server";
import { getDemoPlaylist } from "@/lib/demo-data";
import { fetchPlaylist } from "@/lib/ytmusic";

export const dynamic = "force-dynamic";

export async function GET(
  request: Request,
  context: { params: Promise<{ id: string }> },
) {
  const { id: rawId } = await context.params;
  const id = decodeURIComponent(rawId);
  const { searchParams } = new URL(request.url);
  const forceDemo = searchParams.get("demo") === "1";

  if (forceDemo || id.startsWith("demo-")) {
    const demo = getDemoPlaylist(id);
    if (!demo) {
      return NextResponse.json({ error: "Playlist not found" }, { status: 404 });
    }
    return NextResponse.json(demo);
  }

  try {
    return NextResponse.json(await fetchPlaylist(id));
  } catch (error) {
    console.error("playlist failed", error);
    const demo = getDemoPlaylist(id);
    if (demo) return NextResponse.json(demo);
    return NextResponse.json(
      {
        error:
          error instanceof Error ? error.message : "Could not load playlist",
      },
      { status: 502 },
    );
  }
}
