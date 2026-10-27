import { NextResponse } from "next/server";
import { getDemoHome } from "@/lib/demo-data";
import { fetchHomeFeed } from "@/lib/ytmusic";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url);
  const forceDemo = searchParams.get("demo") === "1";

  if (forceDemo) {
    return NextResponse.json(getDemoHome());
  }

  try {
    const feed = await fetchHomeFeed();
    if (!feed.sections.length) {
      return NextResponse.json(getDemoHome());
    }
    return NextResponse.json(feed);
  } catch (error) {
    console.error("home feed failed", error);
    return NextResponse.json({
      ...getDemoHome(),
      error:
        error instanceof Error
          ? error.message
          : "YouTube Music is unreachable; showing demo library.",
    });
  }
}
