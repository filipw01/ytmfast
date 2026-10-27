import type { Metadata } from "next";
import { Manrope, Sora } from "next/font/google";
import { TooltipProvider } from "@/components/ui/tooltip";
import "./globals.css";

const body = Manrope({
  subsets: ["latin"],
  variable: "--font-body",
});

const display = Sora({
  subsets: ["latin"],
  variable: "--font-display",
});

export const metadata: Metadata = {
  title: "Ytmfast — YouTube Music, native and fast",
  description:
    "A Spotifast-inspired YouTube Music client: library, search, playlists, and a fast player bar.",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" className="dark">
      <body className={`${body.variable} ${display.variable} antialiased`}>
        <TooltipProvider delayDuration={200}>{children}</TooltipProvider>
      </body>
    </html>
  );
}
