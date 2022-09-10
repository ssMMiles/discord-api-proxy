import http from 'k6/http';
import {check, sleep} from 'k6';
export const options = {
    noConnectionReuse: true,
    vus: 50,
    iterations: 50
};

export default function() {
    const params = {
        headers: { 'Authorization': `Bot ${__ENV.TOKEN}` },
    };
    let res = http.get('http://127.0.0.1:8081/api/v9/gateway', params);
  check(res, {
    'success': (r) => {
      console.log(r.status, 2);

      (r.status >= 200 && r.status < 400)
      // (r.status >= 200 && r.status < 400) || (r.status == 429 && r.headers["x-sent-by-proxy"] === "true")
    }
  });

    // let res2 = http.get('http://127.0.0.1:8081/api/v9/guilds/690255418060046348', params);
    // check(res2, {
    //   'success': (r) => {
    //     console.log(r.status, 2);

    //     (r.status >= 200 && r.status < 400) || (r.status == 429 && r.headers["x-sent-by-proxy"] === "true")
    //   }
    // });

    // let res3 = http.get('http://127.0.0.1:8081/api/v9/guilds/690255418060046348/channels', params);
    // check(res3, { 'success': (r) => {
    //   console.log(r.status, 3);

    //   (r.status >= 200 && r.status < 400) || (r.status == 429 && r.headers["x-sent-by-proxy"] === "true")
    // } });
}