import http from 'k6/http';
import { sleep } from 'k6';

export const options = {
  stages: [
    { duration: '1m', target: 750 },
    { duration: '15s', target: 2000 },
    { duration: '5m', target: 2000 },
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
     
  const responses = http.batch([
    gateway, guildId, gateway, guildId
  ]);

  sleep(1);
}