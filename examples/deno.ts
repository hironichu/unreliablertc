import { serve } from "https://deno.land/std/http/server.ts";

await serve((req: Request) => {
	return new Response(Deno.readFileSync("./echo_server.html"), {
		headers: {
			"content-type": "text/html",
		},
	});
}, {
	port: 5050
});