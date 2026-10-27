import { NextResponse } from "next/server";
import { getDemoHome } from "@/lib/demo-data";
import { fetchExploreCharts } from "@/lib/ytmusic";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url);
  if (searchParams.get("demo") === "1") {
    const demo = getDemoHome();
    return NextResponse.json({ source: "demo", sections: demo.sections });
  }

  try {
    const sections = await fetchExploreCharts();
    if (!sections.length) {
      const demo = getDemoHome();
      return NextResponse.json({ source: "demo", sections: demo.sections });
    }
    return NextResponse.json({ source: "live", sections });
  } catch (error) {
    console.error("explore failed", error);
    const demo = getDemoHome();
    return NextResponse.json({
      source: "demo",
      sections: demo.sections,
      error:
        error instanceof Error
          ? error.message
          : "Explore unavailable; showing demo charts.",
    });
  }
}
