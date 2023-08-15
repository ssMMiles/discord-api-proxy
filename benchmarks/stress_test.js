import http from 'k6/http';
import { sleep } from 'k6';

export const options = {
  stages: [
    { duration: '15s', target: 1000 },
    { duration: '15s', target: 2000 },
    { duration: '15m', target: 2000 },
    { duration: '15s', target: 0 }
  ],
};

export default function () {
    const BASE_URL = 'http://127.0.0.1:8080/api/v10'; // make sure this is not production

    let guildId = {
        method: 'GET',
        url: `${BASE_URL}/guilds/690255418060046348`,

        params: {
            headers: {
                Authorization: `Bot ${__ENV.TOKEN}`
            }
        }
    };

    let gateway = {
        method: 'GET',
        url: `${BASE_URL}/gateway`,

        params: {
            headers: {
                Authorization: `Bot ${__ENV.TOKEN}`
            }
        }
    };
    
    let users_me = {
        method: 'GET',
        url: `${BASE_URL}/users/@me`,

        params: {
            headers: {
                Authorization: `Bot ${__ENV.TOKEN}`
            }
        }
    };
    
    let results = http.batch([users_me]);

    for (const response of results) {
        let remaining = response.headers['X-Ratelimit-Remaining'];
        let reset_after = response.headers['X-Ratelimit-Reset-After'];

        if (remaining == 0) {
            sleep(reset_after);
            return;
        } 
    }

    sleep(0.1);
}
